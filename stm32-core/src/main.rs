#![no_std]
#![no_main]

use defmt::*;
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use panic_probe as _;

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    // Initialize the STM32 hardware abstraction layer (HAL)
    let _p = embassy_stm32::init(Default::default());

    info!("System Booting...");
    info!("Sunrise Alarm OS initialized!");

    // Main event loop
    loop {
        info!("Tick: CPU is running...");
        Timer::after(Duration::from_secs(1)).await;
    }
}
