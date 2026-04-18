#![no_std]
#![no_main]

use defmt::*;
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use panic_probe as _;
const WIFI_FIRMWARE: &[u8] = include_bytes!("../firmware/43439A0.bin");
const WIFI_CLM: &[u8] = include_bytes!("../firmware/43439A0_clm.bin");

const INDEX_HTML: &str = r#"
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Alarm</title>
    <style>
        body { font-family: sans-serif; text-align: center; margin-top: 50px; background-color: #f4f4f9; }
        h1 { color: #333; }
        .card { background: white; padding: 20px; border-radius: 10px; display: inline-block; box-shadow: 0 4px 8px rgba(0,0,0,0.1); }
        button { padding: 15px 30px; font-size: 18px; border-radius: 5px; border: none; background: #ff9800; color: white; cursor: pointer; }
    </style>
</head>
<body>
    <div class="card">
        <h1>Sunrise Alarm</h1>
        <h2>Room Temp: 22.5 &deg;C</h2>
        <button>Snooze Alarm</button>
    </div>
</body>
</html>
"#;

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let _p = embassy_rp::init(Default::default());
    info!("Pico W Web Server initialized");
    info!("Wi-Fi Firmware loaded into memory: {} bytes", WIFI_FIRMWARE.len());

    loop {
        info!("Hosting webpage at ");
        info!("(Waiting for a phone to connect)");
        
        Timer::after(Duration::from_secs(5)).await;
    }
}