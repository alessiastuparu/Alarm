#![no_std]
#![no_main]

use defmt::*;
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use panic_probe as _;

use embassy_stm32::bind_interrupts;
use embassy_stm32::mode::Async;
use embassy_stm32::usart::{Config, Uart, UartRx, UartTx};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;

struct ClockState {
    hour: u8,
    minute: u8,
    second: u8,
    alarm_hour: u8,
    alarm_minute: u8,
    alarm_enabled: bool,
    is_ringing: bool,
    ringing_seconds: u32, 
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
});


bind_interrupts!(struct Irqs {
    USART1          => embassy_stm32::usart::InterruptHandler<embassy_stm32::peripherals::USART1>;
    GPDMA1_CHANNEL0 => embassy_stm32::dma::InterruptHandler<embassy_stm32::peripherals::GPDMA1_CH0>;
    GPDMA1_CHANNEL1 => embassy_stm32::dma::InterruptHandler<embassy_stm32::peripherals::GPDMA1_CH1>;
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
                state.hour,
                state.minute,
                state.second,
                state.alarm_hour,
                state.alarm_minute,
                state.alarm_enabled,
                state.is_ringing,
            );
        }
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
                                    info!(
                                        "Snoozed! Next ring at {:02}:{:02}",
                                        state.alarm_hour, state.alarm_minute
                                    );
                                } else {
                                    info!("Snooze ignored — alarm is not ringing.");
                                }
                            }
                            shared_protocol::NetworkCommand::DisableAlarm => {
                                state.alarm_enabled = false;
                                state.is_ringing = false;
                                state.ringing_seconds = 0;
                                info!("Alarm disabled.");
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
        let (is_active, alarm_hour, alarm_minute) = {
            let state = CLOCK_STATE.lock().await;
            (state.alarm_enabled, state.alarm_hour, state.alarm_minute)
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

        let env_msg = shared_protocol::Telemetry::Environment {
            temp_c: 22.5_f32,
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
    )
    .expect("UART init failed");

    let (uart_tx, uart_rx) = uart.split();

    spawner.spawn(uart_listener(uart_rx).unwrap());
    spawner.spawn(telemetry_sender(uart_tx).unwrap());
    spawner.spawn(clock_ticker().unwrap());

    loop {
        Timer::after(Duration::from_secs(60)).await;
    }
}