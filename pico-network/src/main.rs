#![no_std]
#![no_main]

use core::fmt::Write as FmtWrite;

use cyw43_pio::{PioSpi, RM2_CLOCK_DIVIDER};
use defmt::info;
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_net::{Config, StackResources};
use embassy_rp::bind_interrupts;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{DMA_CH0, PIO0};
use embassy_rp::pio::{InterruptHandler, Pio};
use embassy_time::{Duration, Timer};
use embedded_io_async::Write;
use heapless::String;
use panic_probe as _;
use static_cell::StaticCell;

// ─────────────────────────────────────────────────────────────────────────────
// AP credentials
// ─────────────────────────────────────────────────────────────────────────────

const AP_SSID:     &str = "SunriseAlarm";
const AP_PASSWORD: &str = "alarm1234";

// ─────────────────────────────────────────────────────────────────────────────
// Interrupt bindings — PIO only, no DMA_IRQ needed for cyw43-pio 0.9.0
// ─────────────────────────────────────────────────────────────────────────────

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => InterruptHandler<PIO0>;
    UART0_IRQ  => embassy_rp::uart::InterruptHandler<embassy_rp::peripherals::UART0>;
});

// ─────────────────────────────────────────────────────────────────────────────
// Tasks
// ─────────────────────────────────────────────────────────────────────────────

#[embassy_executor::task]
async fn wifi_task(
    runner: cyw43::Runner<'static, Output<'static>, PioSpi<'static, PIO0, 0, DMA_CH0>>,
) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn net_task(mut runner: embassy_net::Runner<'static, cyw43::NetDriver<'static>>) -> ! {
    runner.run().await
}

// ─────────────────────────────────────────────────────────────────────────────
// HTML dashboard
// ─────────────────────────────────────────────────────────────────────────────

const HTML_HEAD: &str = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n\
<!DOCTYPE html><html><head>\
<meta charset='UTF-8'><meta name='viewport' content='width=device-width,initial-scale=1.0'>\
<meta http-equiv='refresh' content='10'><title>Sunrise Alarm</title>\
<style>\
body{font-family:sans-serif;text-align:center;margin-top:50px;background:#f4f4f9;color:#333}\
.card{background:white;padding:30px;border-radius:10px;display:inline-block;\
box-shadow:0 4px 8px rgba(0,0,0,.1);min-width:320px}\
.status{background:#e8f5e9;padding:15px;border-radius:8px;margin:15px 0;text-align:left}\
.status p{margin:6px 0;font-size:15px}\
.wait{color:#aaa;font-style:italic}\
button{padding:10px 20px;font-size:16px;margin:8px;border-radius:5px;\
border:none;cursor:pointer;color:white;transition:.2s}\
button:hover{opacity:.8}\
.gs{background:#4CAF50}.sn{background:#ff9800}.ds{background:#f44336}.sy{background:#2196F3}\
input[type=time]{font-size:20px;padding:5px;margin:10px;\
border:1px solid #ccc;border-radius:5px}\
hr{border:0;border-top:1px solid #eee;margin:20px 0}\
.note{font-size:11px;color:#bbb;margin-top:10px}\
.ap{background:#fff3cd;border:1px solid #ffc107;padding:8px;\
border-radius:6px;font-size:12px;margin-bottom:10px}\
</style></head><body><div class='card'>\
<div class='ap'>Wi-Fi AP: SunriseAlarm &bull; http://192.168.4.1</div>\
<h1>&#127749; Sunrise Alarm</h1><div class='status'>";

const HTML_TAIL: &str = "</div>\
<hr><h3>Alarm Settings</h3>\
<input type='time' id='t'><br>\
<button class='gs' onclick='setAlarm()'>Set Alarm</button>\
<button class='ds' onclick=\"fetch('/disable').then(()=>location.reload())\">Disable</button>\
<hr><h3>Actions</h3>\
<button class='sn' onclick=\"fetch('/snooze').then(()=>location.reload())\">Snooze 5min</button>\
<button class='sy' onclick='syncTime()'>Sync Time</button>\
<p class='note'>Auto-refreshes every 10s</p></div>\
<script>\
function pad(n){return n.toString().padStart(2,'0');}\
function setAlarm(){\
  let t=document.getElementById('t').value;\
  if(t){let p=t.split(':');fetch('/set_alarm?h='+p[0]+'&m='+p[1]).then(()=>{alert('Alarm set for '+t);location.reload();});}\
  else alert('Please select a time first!');}\
function syncTime(){\
  let d=new Date(),h=pad(d.getHours()),m=pad(d.getMinutes()),s=pad(d.getSeconds());\
  fetch('/sync_time?h='+h+'&m='+m+'&s='+s).then(()=>alert('Time synced to '+h+':'+m+':'+s));}\
</script></body></html>";

// ─────────────────────────────────────────────────────────────────────────────
// Main
// ─────────────────────────────────────────────────────────────────────────────

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());
    info!("Pico 2W starting in AP mode...");

    // ── UART TX → STM32 ──────────────────────────────────────────────────────
    let uart_config = embassy_rp::uart::Config::default();
    let mut uart = embassy_rp::uart::Uart::new(
        p.UART0, p.PIN_0, p.PIN_1,
        Irqs, p.DMA_CH1, p.DMA_CH2,
        uart_config,
    );

    // ── CYW43 firmware — must be local vars, not statics ─────────────────────
    let fw    = include_bytes!("../firmware/43439A0.bin");
    let clm   = include_bytes!("../firmware/43439A0_clm.bin");

    // ── CYW43 SPI setup ───────────────────────────────────────────────────────
    let pwr = Output::new(p.PIN_23, Level::Low);
    let cs  = Output::new(p.PIN_25, Level::High);
    let mut pio = Pio::new(p.PIO0, Irqs);

    // RM2_CLOCK_DIVIDER = safe speed for RP2350 (50MHz GSPI)
    // PIN_24 = data, PIN_29 = clock (correct order!)
    let spi = PioSpi::new(
        &mut pio.common, pio.sm0,
        RM2_CLOCK_DIVIDER,
        pio.irq0,
        cs, p.PIN_24, p.PIN_29, p.DMA_CH0,
    );

    static STATE: StaticCell<cyw43::State> = StaticCell::new();
    let state = STATE.init(cyw43::State::new());

    let (net_device, mut control, runner) =
        cyw43::new(state, pwr, spi, fw).await;
    spawner.spawn(wifi_task(runner).unwrap());

    control.init(clm).await;
    control.set_power_management(cyw43::PowerManagementMode::PowerSave).await;

    // ── Start AP — open network for testing ───────────────────────────────────
    info!("Starting open AP '{}'...", AP_SSID);
    control.start_ap_open(AP_SSID, 6).await;
    info!("AP up! Connect to '{}' pw '{}' then open http://192.168.4.1",
          AP_SSID, AP_PASSWORD);

    // Static IP for AP mode
    let config = Config::ipv4_static(embassy_net::StaticConfigV4 {
        address: embassy_net::Ipv4Cidr::new(
            embassy_net::Ipv4Address::new(192, 168, 4, 1), 24,
        ),
        gateway: Some(embassy_net::Ipv4Address::new(192, 168, 4, 1)),
        dns_servers: Default::default(),
    });

    static RESOURCES: StaticCell<StackResources<4>> = StaticCell::new();
    let (stack, runner) = embassy_net::new(
        net_device, config,
        RESOURCES.init(StackResources::new()),
        0x0123_4567_89ab_cdef,
    );
    spawner.spawn(net_task(runner).unwrap());

    Timer::after(Duration::from_secs(2)).await;
    info!("HTTP server ready at http://192.168.4.1");

    // ── HTTP Server ───────────────────────────────────────────────────────────
    let mut rx_buf = [0u8; 1024];
    let mut tx_buf = [0u8; 8192];

    loop {
        let mut socket = embassy_net::tcp::TcpSocket::new(stack, &mut rx_buf, &mut tx_buf);
        socket.set_timeout(Some(Duration::from_secs(10)));

        if socket.accept(80).await.is_err() {
            Timer::after(Duration::from_millis(10)).await;
            continue;
        }

        let mut req_buf = [0u8; 512];
        let n = match socket.read(&mut req_buf).await {
            Ok(n) if n > 0 => n,
            _ => { socket.close(); continue; }
        };
        let request = core::str::from_utf8(&req_buf[..n]).unwrap_or("");

        let mut cmd_buf = [0u8; 32];

        if request.starts_with("GET /snooze") {
            let cmd = shared_protocol::NetworkCommand::SnoozeAlarm;
            if let Ok(data) = postcard::to_slice(&cmd, &mut cmd_buf) {
                let _ = uart.write(data).await;
            }
        } else if request.starts_with("GET /disable") {
            let cmd = shared_protocol::NetworkCommand::DisableAlarm;
            if let Ok(data) = postcard::to_slice(&cmd, &mut cmd_buf) {
                let _ = uart.write(data).await;
            }
        } else if request.starts_with("GET /set_alarm") {
            if let (Some(h), Some(m)) = (
                extract_param(request, "h="),
                extract_param(request, "m="),
            ) {
                if let (Ok(hour), Ok(minute)) = (h.parse::<u8>(), m.parse::<u8>()) {
                    let cmd = shared_protocol::NetworkCommand::SetAlarm { hour, minute };
                    if let Ok(data) = postcard::to_slice(&cmd, &mut cmd_buf) {
                        let _ = uart.write(data).await;
                    }
                }
            }
        } else if request.starts_with("GET /sync_time") {
            if let (Some(h), Some(m), Some(s)) = (
                extract_param(request, "h="),
                extract_param(request, "m="),
                extract_param(request, "s="),
            ) {
                if let (Ok(hour), Ok(minute), Ok(second)) =
                    (h.parse::<u8>(), m.parse::<u8>(), s.parse::<u8>())
                {
                    let cmd = shared_protocol::NetworkCommand::SyncTime { hour, minute, second };
                    if let Ok(data) = postcard::to_slice(&cmd, &mut cmd_buf) {
                        let _ = uart.write(data).await;
                    }
                }
            }
        }

        let mut status: String<128> = String::new();
        core::write!(status, "<p class='wait'>Waiting for STM32 telemetry...</p>").ok();

        let _ = socket.write_all(HTML_HEAD.as_bytes()).await;
        let _ = socket.write_all(status.as_bytes()).await;
        let _ = socket.write_all(HTML_TAIL.as_bytes()).await;
        let _ = socket.flush().await;
        socket.close();
    }
}

fn extract_param<'a>(req: &'a str, key: &str) -> Option<&'a str> {
    let pos = req.find(key)?;
    let after = &req[pos + key.len()..];
    let end = after.find(|c: char| c == '&' || c == ' ' || c == '\r' || c == '\n')
                   .unwrap_or(after.len());
    Some(&after[..end])
}