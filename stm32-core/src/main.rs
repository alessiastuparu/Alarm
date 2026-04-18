#![no_std]
#![no_main]

use defmt::*;
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use panic_probe as _;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq)]
enum AlarmState {
    Idle,            // Normal: shows clock mode, time, temperature
    PreAlarm,        // 5 mins before: LED brightness increases
    Ringing,         // MP3 playing, LEDs light up
    Snoozed,         // Quiet: waits 5 min
    TempAlert,       // Room hot: buzzer activates
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let _p = embassy_stm32::init(Default::default());
    info!("Alarm OS Initialized");

    let mut current_state = AlarmState::Idle;

    loop {
        match current_state {
            AlarmState::Idle => {
                info!("State: IDLE - Displaying Time");




                Timer::after(Duration::from_secs(3)).await;
                current_state = AlarmState::PreAlarm;
            }
            AlarmState::PreAlarm => {
                info!("State: PRE-ALARM - Starting LEDs");

                Timer::after(Duration::from_secs(3)).await;
                current_state = AlarmState::Ringing;
            }
            AlarmState::Ringing => {
                info!("State: RINGING - Playing Audio");

                Timer::after(Duration::from_secs(3)).await;
                current_state = AlarmState::Idle; 
            }
            AlarmState::Snoozed => {
                info!("State: SNOOZED - Waiting 5 Minutes");
                Timer::after(Duration::from_secs(5)).await;
                current_state = AlarmState::Ringing;
            }
            AlarmState::TempAlert => {
                warn!("State: TEMP ALERT - Triggering Buzzer");
                Timer::after(Duration::from_secs(2)).await;
                current_state = AlarmState::Idle;
            }
        }
    }
}