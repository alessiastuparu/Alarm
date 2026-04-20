#![no_std]
#![no_main]

use defmt::*;
use defmt_rtt as _;
use embassy_executor::Spawner;
use panic_probe as _;

use embassy_stm32::usart::{Config, Uart};
use embassy_stm32::bind_interrupts;
use embassy_stm32::peripherals::{USART1, GPDMA1_CH0, GPDMA1_CH1};

bind_interrupts!(struct Irqs{
    USART1 => embassy_stm32::usart::InterruptHandler<embassy_stm32::peripherals::USART1>;
});

#[embassy_executor::task]
async fn uart_listener(mut uart: Uart<'static, USART1, GPDMA1_CH0, GPDMA1_CH1>){
    let mut rx_buf = [0u8; 32];
    info!("Stm32 listener started, waiting for Pico W commands");

    loop{
        match uart.read_until_idle(&mut rx_buf).await{
            Ok(len) if len > 0 => {
                if let Ok(cmd) = postcard::from_bytes::<shared_protocol::NetworkCommand>(&rx_buf[..len]) {
                    match cmd {
                        shared_protocol::NetworkCommand::SnoozeAlarm => {
                            info!("Command: Snooze starting, Buzzer is pausing");
                        }
                        shared_protocol::NetworkCommand::DisableAlarm => {
                            info!("Command: Alarm Disabled");
                        }
                        shared_protocol::NetworkCommand::SetAlarm { hour, minute } => {
                            info!("Command: Alarm set for {:02}:{:02}", hour, minute);
                        }
                        shared_protocol::NetworkCommand::SyncTime { hour, minute, second } => {
                            info!("Command: STM32 internal clock synced to {:02}:{:02}:{:02}", hour, minute, second);
                        }
                    }
                }
            } 
            Ok(_) => {}
            Err(_) => {
                warn!("UART read error");
            }
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

    loop{
        embassy_time::Timer::after(embassy_time::Duration::from_secs(5)).await;
        info!("STM32 main loop running");
    }
}