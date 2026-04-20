#![no_std]
#![no_main]

use defmt::*;
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use panic_probe as _;

use cyw43_pio::PioSpi;
use embassy_net::{Config, Stack, StackResources};
use embassy_rp::bind_interrupts;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{DMA_CH0, PIO0};
use embassy_rp::pio::{InterruptHandler, Pio};
use static_cell::StaticCell;

use embedded_io_async::Write;

const WIFI_NETWORK: &str = "Targu Jiu";
const WIFI_PASSWORD: &str = "Gorj13579!";
const WIFI_FIRMWARE: &[u8] = include_bytes!("../firmware/43439A0.bin");
const WIFI_CLM: &[u8] = include_bytes!("../firmware/43439A0_clm.bin");

#[allow(dead_code)]
const INDEX_HTML: &str = r#"
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Sunrise Alarm Control</title>
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

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => InterruptHandler<PIO0>;
    UART0_IRQ => embassy_rp::uart::InterruptHandler<embassy_rp::peripherals::UART0>;
});

#[embassy_executor::task]
async fn wifi_task(
    runner: cyw43::Runner<'static, Output<'static>, PioSpi<'static, PIO0, 0, DMA_CH0>>
) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn net_task(stack: &'static Stack<cyw43::NetDriver<'static>>) -> ! {
    stack.run().await
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());
    info!("Pico W Web Server initialized");

    let uart_config = embassy_rp::uart::Config::default();
    let mut uart = embassy_rp::uart::Uart::new(p.UART0, p.PIN_0, p.PIN_1, Irqs, p.DMA_CH1, p.DMA_CH2, uart_config);

    let pwr = Output::new(p.PIN_23, Level::Low);
    let cs = Output::new(p.PIN_25, Level::High);
    let mut pio = Pio::new(p.PIO0, Irqs);
    let spi = PioSpi::new(&mut pio.common, pio.sm0, pio.irq0, cs, p.PIN_24, p.PIN_29, p.DMA_CH0);

    static STATE: StaticCell<cyw43::State> = StaticCell::new();
    let state = STATE.init(cyw43::State::new());
    let (net_device, mut control, wifi_runner) = cyw43::new(state, pwr, spi, WIFI_FIRMWARE).await;
    spawner.spawn(wifi_task(wifi_runner)).unwrap();

    control.init(WIFI_CLM).await;
    control.set_power_management(cyw43::PowerManagementMode::PowerSave).await;

    let config = Config::dhcpv4(Default::default());
    static RESOURCES: StaticCell<StackResources<2>> = StaticCell::new();
    let resources = RESOURCES.init(StackResources::<2>::new());
    static STACK: StaticCell<Stack<cyw43::NetDriver<'static>>> = StaticCell::new();
    let stack = STACK.init(Stack::new(net_device, config, resources, 1234));
    spawner.spawn(net_task(stack)).unwrap();

    info!("Connecting to Wi-Fi...");
    loop {
        if let Ok(_) = control.join_wpa2(WIFI_NETWORK, WIFI_PASSWORD).await { break; }
        Timer::after(Duration::from_secs(1)).await;
    }
    
    let ip = stack.config_v4().unwrap().address.address();
    info!("Pico W IP Address: {}", ip);

    let mut rx_buffer = [0; 1024];
    let mut tx_buffer = [0; 4096];

    loop {
        let mut socket = embassy_net::tcp::TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);
        socket.set_timeout(Some(Duration::from_secs(10)));

        if let Ok(_) = socket.accept(80).await {
            let mut buf = [0; 1024];
            if let Ok(n) = socket.read(&mut buf).await {
                let request = core::str::from_utf8(&buf[..n]).unwrap_or("");

                if request.contains("GET /snooze") {
    info!("Snooze pressed - Sending command to STM32");
    
    let cmd = shared_protocol::NetworkCommand::SnoozeAlarm;
    
    let mut send_buf = [0u8; 32];
    if let Ok(data) = postcard::to_slice(&cmd, &mut send_buf) {
        let _ = uart.write(data).await;
    }
}

                let headers = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n";
                let _ = socket.write_all(headers.as_bytes()).await;
                let _ = socket.write_all(INDEX_HTML.as_bytes()).await;
                let _ = socket.flush().await;
            }
            socket.close();
        }
    }
}