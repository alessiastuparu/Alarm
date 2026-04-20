#![no_std]
#![no_main]

use defmt::*;
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use panic_probe as _;

use embassy_stm32::usart::{Config, Uart};
use embassy_stm32::bind_interrupts;
use embassy_stm32::peripherals::{USART1, GPDMA1_CH0, GPDMA1_CH1};

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
}

static CLOCK_STATE: Mutex<CriticalSectionRawMutex, ClockState> = Mutex::new(ClockState {
    hour: 0, minute: 0, second: 0,
    alarm_hour: 7, alarm_minute: 0, 
    alarm_enabled: false,
    is_ringing: false,
});

bind_interrupts!(struct Irqs{
    USART1 => embassy_stm32::usart::InterruptHandler<embassy_stm32::peripherals::USART1>;
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
            if state.hour == state.alarm_hour && state.minute == state.alarm_minute && state.second == 0 {
                state.is_ringing = true;
                info!("Wake up!");
            }
        }

        if state.second % 10 == 0 {
            info!("Current internal time: {:02}:{:02}:{:02}", state.hour, state.minute, state.second);
        }
    }
}

#[embassy_executor::task]
async fn uart_listener(mut uart: Uart<'static, USART1, GPDMA1_CH0, GPDMA1_CH1>){
    let mut rx_buf = [0u8; 32];
    info!("Stm32 listener started, waiting for Pico W commands");

    loop{
        match uart.read_until_idle(&mut rx_buf).await{
            Ok(len) if len > 0 => {
                if let Ok(cmd) = postcard::from_bytes::<shared_protocol::NetworkCommand>(&rx_buf[..len]) {
                    
                    let mut state = CLOCK_STATE.lock().await;

                    match cmd {
                        shared_protocol::NetworkCommand::SnoozeAlarm => {
                            state.is_ringing = false; 
                            state.alarm_minute = (state.minute + 5) % 60;
                            if state.minute + 5 >= 60 { state.alarm_hour = (state.alarm_hour + 1) % 24; }
                            info!("Snoozed! Alarm will ring again at {:02}:{:02}", state.alarm_hour, state.alarm_minute);
                        }
                        shared_protocol::NetworkCommand::DisableAlarm => {
                            state.alarm_enabled = false;
                            state.is_ringing = false;
                            info!("Alarm Disabled.");
                        }
                        shared_protocol::NetworkCommand::SetAlarm { hour, minute } => {
                            state.alarm_hour = hour;
                            state.alarm_minute = minute;
                            state.alarm_enabled = true;
                            info!("Alarm successfully set for {:02}:{:02}", hour, minute);
                        }
                        shared_protocol::NetworkCommand::SyncTime { hour, minute, second } => {
                            state.hour = hour;
                            state.minute = minute;
                            state.second = second;
                            info!("Clock synced to {:02}:{:02}:{:02}", hour, minute, second);
                        }
                    }
                }
            } 
            Ok(_) => {}
            Err(_) => { warn!("UART read error"); }
        }
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner){
    let p = embassy_stm32::init(Default::default());
    info!("Alarm OS initialized");

    let mut uart_config = Config::default();
    uart_config.baudrate = 115200;

    let uart = Uart::new(
        p.USART1,
        p.PA10,
        p.PA9,
        Irqs,
        p.GPDMA1_CH0,
        p.GPDMA1_CH1,
        uart_config
    ).expect("UART configuration failed");

    spawner.spawn(uart_listener(uart)).unwrap();
    spawner.spawn(clock_ticker()).unwrap();

    loop{
        Timer::after(Duration::from_secs(60)).await;
    }
}