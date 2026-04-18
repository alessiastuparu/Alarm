#![no_std]

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum NetworkCommand {
    SetAlarm { hour: u8, minute: u8 },
    DisableAlarm,
    SnoozeAlarm,
    SyncTime { hour: u8, minute: u8, second: u8 },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Telemetry {
    Environment { temp_c: f32, humidity: f32 },
    AlarmStatus { is_active: bool, hour: u8, minute: u8 },
}