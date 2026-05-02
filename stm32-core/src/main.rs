#![no_std]
#![no_main]

use defmt::{info, warn};
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use panic_probe as _;

use embassy_stm32::bind_interrupts;
use embassy_stm32::exti::ExtiInput;
use embassy_stm32::gpio::{Level, Output, Pull, Speed};
use embassy_stm32::mode::Async;
use embassy_stm32::spi::{self, Spi};
use embassy_stm32::time::Hertz;
use embassy_stm32::usart::{Config, Uart, UartRx, UartTx};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;

use embedded_graphics::mono_font::ascii::{FONT_9X18_BOLD, FONT_6X10};
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::text::Text;
use embedded_graphics::primitives::{Rectangle, PrimitiveStyleBuilder};

use heapless::String;
use core::fmt::Write as FmtWrite;

struct ClockState {
    hour: u8,
    minute: u8,
    second: u8,
    alarm_hour: u8,
    alarm_minute: u8,
    alarm_enabled: bool,
    is_ringing: bool,
    ringing_seconds: u32,
    temp_tenths: i16,
}

static CLOCK_STATE: Mutex<CriticalSectionRawMutex, ClockState> = Mutex::new(ClockState {
    hour: 0,
    minute: 0,
    second: 0,
    alarm_hour: 7,
    alarm_minute: 0,
    alarm_enabled: false,
    is_ringing: false,
    ringing_seconds: 0,
    temp_tenths: 225, 
});


bind_interrupts!(struct Irqs {
    USART1          => embassy_stm32::usart::InterruptHandler<embassy_stm32::peripherals::USART1>;
    GPDMA1_CHANNEL0 => embassy_stm32::dma::InterruptHandler<embassy_stm32::peripherals::GPDMA1_CH0>;
    GPDMA1_CHANNEL1 => embassy_stm32::dma::InterruptHandler<embassy_stm32::peripherals::GPDMA1_CH1>;
    GPDMA1_CHANNEL2 => embassy_stm32::dma::InterruptHandler<embassy_stm32::peripherals::GPDMA1_CH2>;
    GPDMA1_CHANNEL3 => embassy_stm32::dma::InterruptHandler<embassy_stm32::peripherals::GPDMA1_CH3>;
    EXTI8           => embassy_stm32::exti::InterruptHandler<embassy_stm32::interrupt::typelevel::EXTI8>;
    EXTI5           => embassy_stm32::exti::InterruptHandler<embassy_stm32::interrupt::typelevel::EXTI5>;
});


#[embassy_executor::task]
async fn clock_ticker() {
    loop {
        Timer::after(Duration::from_secs(1)).await;

        let mut state = CLOCK_STATE.lock().await;

        state.second += 1;
        if state.second >= 60 {
            state.second = 0;
            state.minute += 1;
            if state.minute >= 60 {
                state.minute = 0;
                state.hour += 1;
                if state.hour >= 24 {
                    state.hour = 0;
                }
            }
        }

        if state.alarm_enabled && !state.is_ringing {
            if state.hour == state.alarm_hour
                && state.minute == state.alarm_minute
                && state.second == 0
            {
                state.is_ringing = true;
                state.ringing_seconds = 0;
                info!("ALARM TRIGGERED — Wake up!");
            }
        }

        if state.is_ringing {
            state.ringing_seconds += 1;
            if state.ringing_seconds >= 300 {
                state.is_ringing = false;
                state.alarm_enabled = false;
                state.ringing_seconds = 0;
                info!("Alarm auto-cancelled after 5 minutes.");
            }
        }

        if state.second % 10 == 0 {
            info!(
                "Time {:02}:{:02}:{:02} | Alarm {:02}:{:02} enabled={} ringing={}",
                state.hour, state.minute, state.second,
                state.alarm_hour, state.alarm_minute,
                state.alarm_enabled, state.is_ringing,
            );
        }
    }
}

#[embassy_executor::task]
async fn buzzer_driver(mut buzzer_pin: Output<'static>) {
    info!("Buzzer driver started.");
    loop {
        let ringing = {
            let state = CLOCK_STATE.lock().await;
            state.is_ringing
        };
        if ringing {
            buzzer_pin.set_high();
            Timer::after(Duration::from_millis(500)).await;
            buzzer_pin.set_low();
            Timer::after(Duration::from_millis(500)).await;
        } else {
            buzzer_pin.set_low();
            Timer::after(Duration::from_millis(100)).await;
        }
    }
}

#[embassy_executor::task]
async fn button_snooze(mut button: ExtiInput<'static, Async>) {
    info!("Snooze button task started.");
    loop {
        button.wait_for_falling_edge().await;
        Timer::after(Duration::from_millis(500)).await; 

        let mut state = CLOCK_STATE.lock().await;
        if state.is_ringing {
            state.is_ringing = false;
            state.ringing_seconds = 0;
            let total_minutes = state.alarm_minute as u16 + 5;
            state.alarm_minute = (total_minutes % 60) as u8;
            if total_minutes >= 60 {
                state.alarm_hour = (state.alarm_hour + 1) % 24;
            }
            info!("Physical snooze! Next ring at {:02}:{:02}", state.alarm_hour, state.alarm_minute);
        } else {
            info!("Snooze button pressed but alarm is not ringing — ignored.");
        }
    }
}

#[embassy_executor::task]
async fn button_cancel(mut button: ExtiInput<'static, Async>) {
    info!("Cancel button task started.");
    loop {
        button.wait_for_falling_edge().await;
        Timer::after(Duration::from_millis(500)).await; 

        let mut state = CLOCK_STATE.lock().await;
        if state.is_ringing || state.alarm_enabled {
            state.alarm_enabled = false;
            state.is_ringing = false;
            state.ringing_seconds = 0;
            info!("Physical cancel! Alarm fully disabled.");
        } else {
            info!("Cancel button pressed but no alarm is active — ignored.");
        }
    }
}

#[embassy_executor::task]
async fn display_task(
    mut display: st7735_lcd::ST7735<
        Spi<'static, embassy_stm32::mode::Async, embassy_stm32::spi::mode::Master>,
        Output<'static>,
        Output<'static>,
    >,
) {
    info!("Display task started.");

    display.clear(Rgb565::BLACK).ok();

    let time_style = MonoTextStyle::new(&FONT_9X18_BOLD, Rgb565::WHITE);
    let label_style = MonoTextStyle::new(&FONT_6X10, Rgb565::CSS_GRAY);
    let alarm_on_style = MonoTextStyle::new(&FONT_6X10, Rgb565::GREEN);
    let alarm_ring_style = MonoTextStyle::new(&FONT_6X10, Rgb565::RED);
    let temp_style = MonoTextStyle::new(&FONT_6X10, Rgb565::CSS_LIGHT_CYAN);

    loop {
        let (hour, minute, second, alarm_hour, alarm_minute, alarm_enabled, is_ringing, temp_tenths) = {
            let s = CLOCK_STATE.lock().await;
            (s.hour, s.minute, s.second, s.alarm_hour, s.alarm_minute,
             s.alarm_enabled, s.is_ringing, s.temp_tenths)
        };

        let clear_style = PrimitiveStyleBuilder::new()
            .fill_color(Rgb565::BLACK)
            .build();
        Rectangle::new(Point::new(0, 0), Size::new(128, 160))
            .into_styled(clear_style)
            .draw(&mut display)
            .ok();

        Text::new("TIME", Point::new(4, 16), label_style)
            .draw(&mut display).ok();

        let mut time_str: String<12> = String::new();
        core::write!(time_str, "{:02}:{:02}:{:02}", hour, minute, second).ok();
        Text::new(time_str.as_str(), Point::new(4, 40), time_style)
            .draw(&mut display).ok();

        Text::new("ALARM", Point::new(4, 64), label_style)
            .draw(&mut display).ok();

        let mut alarm_str: String<20> = String::new();
        core::write!(alarm_str, "{:02}:{:02}", alarm_hour, alarm_minute).ok();

        let (status_str, status_style) = if is_ringing {
            ("  RINGING!", alarm_ring_style)
        } else if alarm_enabled {
            ("  SET", alarm_on_style)
        } else {
            ("  OFF", label_style)
        };

        Text::new(alarm_str.as_str(), Point::new(4, 82), time_style)
            .draw(&mut display).ok();
        Text::new(status_str, Point::new(60, 82), status_style)
            .draw(&mut display).ok();

        Text::new("TEMP", Point::new(4, 110), label_style)
            .draw(&mut display).ok();

        let mut temp_str: String<16> = String::new();
        let temp_whole = temp_tenths / 10;
        let temp_frac = (temp_tenths % 10).abs();
        core::write!(temp_str, "{}.{} C", temp_whole, temp_frac).ok();
        Text::new(temp_str.as_str(), Point::new(4, 128), temp_style)
            .draw(&mut display).ok();

        Timer::after(Duration::from_secs(1)).await;
    }
}

#[embassy_executor::task]
async fn uart_listener(mut uart_rx: UartRx<'static, Async>) {
    let mut rx_buf = [0u8; 32];
    info!("STM32 UART listener ready — awaiting Pico commands...");
    loop {
        match uart_rx.read_until_idle(&mut rx_buf).await {
            Ok(len) if len > 0 => {
                match postcard::from_bytes::<shared_protocol::NetworkCommand>(&rx_buf[..len]) {
                    Ok(cmd) => {
                        let mut state = CLOCK_STATE.lock().await;
                        match cmd {
                            shared_protocol::NetworkCommand::SyncTime { hour, minute, second } => {
                                state.hour = hour;
                                state.minute = minute;
                                state.second = second;
                                info!("Clock synced to {:02}:{:02}:{:02}", hour, minute, second);
                            }
                            shared_protocol::NetworkCommand::SetAlarm { hour, minute } => {
                                state.alarm_hour = hour;
                                state.alarm_minute = minute;
                                state.alarm_enabled = true;
                                state.is_ringing = false;
                                state.ringing_seconds = 0;
                                info!("Alarm set for {:02}:{:02}", hour, minute);
                            }
                            shared_protocol::NetworkCommand::SnoozeAlarm => {
                                if state.is_ringing {
                                    state.is_ringing = false;
                                    state.ringing_seconds = 0;
                                    let total_minutes = state.alarm_minute as u16 + 5;
                                    state.alarm_minute = (total_minutes % 60) as u8;
                                    if total_minutes >= 60 {
                                        state.alarm_hour = (state.alarm_hour + 1) % 24;
                                    }
                                    info!("Web snooze! Next ring at {:02}:{:02}", state.alarm_hour, state.alarm_minute);
                                } else {
                                    info!("Web snooze ignored — alarm not ringing.");
                                }
                            }
                            shared_protocol::NetworkCommand::DisableAlarm => {
                                state.alarm_enabled = false;
                                state.is_ringing = false;
                                state.ringing_seconds = 0;
                                info!("Web disable! Alarm disabled.");
                            }
                        }
                    }
                    Err(_) => warn!("Received unrecognised bytes from Pico — ignored."),
                }
            }
            Ok(_) => {}
            Err(_) => warn!("UART RX error."),
        }
    }
}

#[embassy_executor::task]
async fn telemetry_sender(mut uart_tx: UartTx<'static, Async>) {
    Timer::after(Duration::from_secs(3)).await;
    info!("Telemetry sender started.");
    let mut buf = [0u8; 32];
    loop {
        let (is_active, alarm_hour, alarm_minute, temp_tenths) = {
            let state = CLOCK_STATE.lock().await;
            (state.alarm_enabled, state.alarm_hour, state.alarm_minute, state.temp_tenths)
        };

        let status_msg = shared_protocol::Telemetry::AlarmStatus {
            is_active,
            hour: alarm_hour,
            minute: alarm_minute,
        };
        if let Ok(data) = postcard::to_slice(&status_msg, &mut buf) {
            let len_byte = [data.len() as u8];
            let _ = uart_tx.write(&len_byte).await;
            let _ = uart_tx.write(data).await;
        }

        Timer::after(Duration::from_millis(50)).await;

        let temp_c = temp_tenths as f32 / 10.0;
        let env_msg = shared_protocol::Telemetry::Environment {
            temp_c,
            humidity: 55.0_f32, 
        };
        if let Ok(data) = postcard::to_slice(&env_msg, &mut buf) {
            let len_byte = [data.len() as u8];
            let _ = uart_tx.write(&len_byte).await;
            let _ = uart_tx.write(data).await;
        }

        info!("Telemetry sent to Pico.");
        Timer::after(Duration::from_secs(10)).await;
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_stm32::init(Default::default());
    info!("STM32 Alarm OS initialised.");

    let mut uart_config = Config::default();
    uart_config.baudrate = 115200;
    let uart = Uart::new(
        p.USART1,
        p.PA10, 
        p.PA9,  
        p.GPDMA1_CH0,
        p.GPDMA1_CH1,
        Irqs,
        uart_config,
    ).expect("UART init failed");
    let (uart_tx, uart_rx) = uart.split();

    let buzzer = Output::new(p.PB0, Level::Low, Speed::Low);

    let snooze_btn = ExtiInput::new(p.PA8, p.EXTI8, Pull::Up, Irqs);
    let cancel_btn = ExtiInput::new(p.PB5, p.EXTI5, Pull::Up, Irqs);

    let mut spi_config = spi::Config::default();
    spi_config.frequency = Hertz(2_000_000); 

    let spi = Spi::new(
        p.SPI1,
        p.PA5,         
        p.PA7,         
        p.PA6,         
        p.GPDMA1_CH2,  
        p.GPDMA1_CH3,  
        Irqs,
        spi_config,
    );

    let dc  = Output::new(p.PC7, Level::Low,  Speed::High); 
    let rst = Output::new(p.PC6, Level::High, Speed::High); 

    let mut display = st7735_lcd::ST7735::new(spi, dc, rst, false, false, 128, 160);

    display.init(&mut embassy_time::Delay).ok();
    display.set_orientation(&st7735_lcd::Orientation::Portrait).ok();
    info!("Display initialised.");

    spawner.spawn(clock_ticker().unwrap());
    spawner.spawn(buzzer_driver(buzzer).unwrap());
    spawner.spawn(button_snooze(snooze_btn).unwrap());
    spawner.spawn(button_cancel(cancel_btn).unwrap());
    spawner.spawn(display_task(display).unwrap());
    spawner.spawn(uart_listener(uart_rx).unwrap());
    spawner.spawn(telemetry_sender(uart_tx).unwrap());

    loop {
        Timer::after(Duration::from_secs(60)).await;
    }
}