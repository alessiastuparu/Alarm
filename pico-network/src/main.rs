#![no_std]
#![no_main]

use defmt::{info, warn};
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use panic_probe as _;

use cyw43_pio::PioSpi;
use embassy_net::{Config, StackResources};
use embassy_rp::bind_interrupts;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{DMA_CH0, PIO0};
use embassy_rp::pio::{InterruptHandler, Pio};
use core::fmt::Write as FmtWrite;
use embedded_io_async::Write;
use embassy_rp::uart::Async;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use heapless::String;
use static_cell::StaticCell;


const WIFI_NETWORK: &str = "DIGI-22uE";
const WIFI_PASSWORD: &str = "U453DB2QYNcW";
const WIFI_FIRMWARE: &[u8] = include_bytes!("../firmware/43439A0.bin");
const WIFI_CLM: &[u8] = include_bytes!("../firmware/43439A0_clm.bin");


struct PicoTelemetry {
    temp_whole:     i16,
    temp_frac:      u16,
    humidity_whole: i16,
    humidity_frac:  u16,
    alarm_active:   bool,
    alarm_hour:     u8,
    alarm_minute:   u8,
    has_data:       bool,
}

static TELEMETRY: Mutex<CriticalSectionRawMutex, PicoTelemetry> = Mutex::new(PicoTelemetry {
    temp_whole:     0,
    temp_frac:      0,
    humidity_whole: 0,
    humidity_frac:  0,
    alarm_active:   false,
    alarm_hour:     0,
    alarm_minute:   0,
    has_data:       false,
});

fn split_float(val: f32) -> (i16, u16) {
    let whole = val as i16;
    let frac  = ((val - whole as f32).abs() * 10.0) as u16;
    (whole, frac)
}


const HTML_HEAD: &str = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n\
<!DOCTYPE html>\
<html lang='en'>\
<head>\
<meta charset='UTF-8'>\
<meta name='viewport' content='width=device-width, initial-scale=1.0'>\
<meta http-equiv='refresh' content='10'>\
<title>Sunrise Alarm</title>\
<style>\
body{font-family:sans-serif;text-align:center;margin-top:50px;background:#f4f4f9;color:#333}\
.card{background:white;padding:30px;border-radius:10px;display:inline-block;\
box-shadow:0 4px 8px rgba(0,0,0,.1);min-width:320px}\
.status{background:#e8f5e9;padding:15px;border-radius:8px;margin:15px 0;text-align:left}\
.status p{margin:6px 0;font-size:15px}\
.on{color:white;background:#4CAF50;padding:2px 8px;border-radius:4px;font-size:13px}\
.off{color:white;background:#9e9e9e;padding:2px 8px;border-radius:4px;font-size:13px}\
.wait{color:#aaa;font-style:italic}\
button{padding:10px 20px;font-size:16px;margin:8px;border-radius:5px;border:none;\
cursor:pointer;color:white;transition:.2s}\
button:hover{opacity:.8}\
.gs{background:#4CAF50}.sn{background:#ff9800}.ds{background:#f44336}.sy{background:#2196F3}\
input[type=time]{font-size:20px;padding:5px;margin:10px;border:1px solid #ccc;border-radius:5px}\
hr{border:0;border-top:1px solid #eee;margin:20px 0}\
.note{font-size:11px;color:#bbb;margin-top:10px}\
</style></head><body><div class='card'><h1>Sunrise Alarm</h1><div class='status'>";

const HTML_TAIL: &str = "</div>\
<hr><h3>Alarm Settings</h3>\
<input type='time' id='t'><br>\
<button class='gs' onclick='setAlarm()'>Set Alarm</button>\
<button class='ds' onclick=\"fetch('/disable').then(()=>location.reload())\">Disable</button>\
<hr><h3>Actions</h3>\
<button class='sn' onclick=\"fetch('/snooze').then(()=>location.reload())\">Snooze (5 min)</button>\
<button class='sy' onclick='syncTime()'>Sync Time to Phone</button>\
<p class='note'>Page auto-refreshes every 10 s.</p></div>\
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


bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => InterruptHandler<PIO0>;
    UART0_IRQ  => embassy_rp::uart::InterruptHandler<embassy_rp::peripherals::UART0>;
});


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


#[embassy_executor::task]
async fn telemetry_listener(mut uart_rx: embassy_rp::uart::UartRx<'static, Async>) -> ! {
    let mut rx_buf = [0u8; 32];
    info!("Pico telemetry listener ready.");

    loop {
        let mut len_buf = [0u8; 1];
        if uart_rx.read(&mut len_buf).await.is_err() {
            Timer::after(Duration::from_millis(10)).await;
            continue;
        }

        let len = len_buf[0] as usize;
        if len == 0 || len > rx_buf.len() {
            warn!("Telemetry: bad length byte {}", len_buf[0]);
            continue;
        }

        match uart_rx.read(&mut rx_buf[..len]).await {
            Ok(()) => {}
            Err(_) => {
                warn!("Telemetry: payload read error.");
                continue;
            }
        }

        match postcard::from_bytes::<shared_protocol::Telemetry>(&rx_buf[..len]) {
            Ok(msg) => {
                let mut tel = TELEMETRY.lock().await;
                tel.has_data = true;
                match msg {
                    shared_protocol::Telemetry::Environment { temp_c, humidity } => {
                        let (tw, tf) = split_float(temp_c);
                        let (hw, hf) = split_float(humidity);
                        tel.temp_whole     = tw;
                        tel.temp_frac      = tf;
                        tel.humidity_whole = hw;
                        tel.humidity_frac  = hf;
                        info!("Env: {}.{}C  {}.{}%RH", tw, tf, hw, hf);
                    }
                    shared_protocol::Telemetry::AlarmStatus { is_active, hour, minute } => {
                        tel.alarm_active = is_active;
                        tel.alarm_hour   = hour;
                        tel.alarm_minute = minute;
                        info!("Alarm: active={} {:02}:{:02}", is_active, hour, minute);
                    }
                }
            }
            Err(_) => warn!("Telemetry: parse failed."),
        }
    }
}


#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());
    info!("Pico W initialising...");

    let uart_config = embassy_rp::uart::Config::default();
    let uart = embassy_rp::uart::Uart::new(
        p.UART0,
        p.PIN_0,   
        p.PIN_1, 
        Irqs,
        p.DMA_CH1, 
        p.DMA_CH2, 
        uart_config,
    );
    let (mut uart_tx, uart_rx) = uart.split();

    spawner.spawn(telemetry_listener(uart_rx).unwrap());

    let pwr = Output::new(p.PIN_23, Level::Low);
    let cs  = Output::new(p.PIN_25, Level::High);
    let mut pio = Pio::new(p.PIO0, Irqs);
    let clock_div = fixed::FixedU32::<typenum::U8>::from_num(1);
    let spi = PioSpi::new(
        &mut pio.common, pio.sm0, clock_div, pio.irq0,
        cs, p.PIN_29, p.PIN_24, p.DMA_CH0,
    );

    static STATE: StaticCell<cyw43::State> = StaticCell::new();
    let state = STATE.init(cyw43::State::new());
    let (net_device, mut control, wifi_runner) =
        cyw43::new(state, pwr, spi, WIFI_FIRMWARE).await;
    spawner.spawn(wifi_task(wifi_runner).unwrap());

    control.init(WIFI_CLM).await;
    control.set_power_management(cyw43::PowerManagementMode::PowerSave).await;

    let config = Config::dhcpv4(Default::default());
    static RESOURCES: StaticCell<StackResources<2>> = StaticCell::new();
    let resources = RESOURCES.init(StackResources::<2>::new());
    let (stack, runner) = embassy_net::new(net_device, config, resources, 1234);
    spawner.spawn(net_task(runner).unwrap());

    info!("Connecting to Wi-Fi...");
    loop {
        if control.join(WIFI_NETWORK, cyw43::JoinOptions::new(WIFI_PASSWORD.as_bytes())).await.is_ok() {
            break;
        }
        Timer::after(Duration::from_secs(1)).await;
    }

    let ip = stack.config_v4().unwrap().address.address();
    info!("Connected! Dashboard at http://{}", ip);

    let mut rx_buffer = [0u8; 1024];
    let mut tx_buffer = [0u8; 8192];

    loop {
        let mut socket = embassy_net::tcp::TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);
        socket.set_timeout(Some(Duration::from_secs(10)));

        if socket.accept(80).await.is_err() {
            continue;
        }

        let mut req_buf = [0u8; 512];
        let n = match socket.read(&mut req_buf).await {
            Ok(n) if n > 0 => n,
            _ => { socket.close(); continue; }
        };
        let request = core::str::from_utf8(&req_buf[..n]).unwrap_or("");

        let mut send_buf = [0u8; 32];

        if request.starts_with("GET /snooze") {
            info!("Web: Snooze");
            let cmd = shared_protocol::NetworkCommand::SnoozeAlarm;
            if let Ok(data) = postcard::to_slice(&cmd, &mut send_buf) {
                let _ = uart_tx.write(data).await;
            }
        } else if request.starts_with("GET /disable") {
            info!("Web: Disable");
            let cmd = shared_protocol::NetworkCommand::DisableAlarm;
            if let Ok(data) = postcard::to_slice(&cmd, &mut send_buf) {
                let _ = uart_tx.write(data).await;
            }
        } else if request.starts_with("GET /set_alarm") {
            if let (Some(h_s), Some(m_s)) = (
                extract_param(request, "h="),
                extract_param(request, "m="),
            ) {
                if let (Ok(hour), Ok(minute)) = (h_s.parse::<u8>(), m_s.parse::<u8>()) {
                    info!("Web: Set alarm {:02}:{:02}", hour, minute);
                    let cmd = shared_protocol::NetworkCommand::SetAlarm { hour, minute };
                    if let Ok(data) = postcard::to_slice(&cmd, &mut send_buf) {
                        let _ = uart_tx.write(data).await;
                    }
                }
            }
        } else if request.starts_with("GET /sync_time") {
            if let (Some(h_s), Some(m_s), Some(s_s)) = (
                extract_param(request, "h="),
                extract_param(request, "m="),
                extract_param(request, "s="),
            ) {
                if let (Ok(hour), Ok(minute), Ok(second)) =
                    (h_s.parse::<u8>(), m_s.parse::<u8>(), s_s.parse::<u8>())
                {
                    info!("Web: Sync time {:02}:{:02}:{:02}", hour, minute, second);
                    let cmd = shared_protocol::NetworkCommand::SyncTime { hour, minute, second };
                    if let Ok(data) = postcard::to_slice(&cmd, &mut send_buf) {
                        let _ = uart_tx.write(data).await;
                    }
                }
            }
        }

        let mut status_block: String<512> = String::new();
        {
            let tel = TELEMETRY.lock().await;
            if tel.has_data {
                let badge = if tel.alarm_active { "<span class='on'>ACTIVE</span>" }
                            else               { "<span class='off'>OFF</span>"    };
                write!(
                    status_block,
                    "<p>Temperature: {}.{} C</p>\
                     <p>Humidity: {}.{}%</p>\
                     <p>Alarm: {:02}:{:02} &nbsp;{}</p>",
                    tel.temp_whole, tel.temp_frac,
                    tel.humidity_whole, tel.humidity_frac,
                    tel.alarm_hour, tel.alarm_minute,
                    badge,
                ).ok();
            } else {
                write!(
                    status_block,
                    "<p class='wait'>Waiting for STM32 telemetry...</p>"
                ).ok();
            }
        }

        let _ = socket.write_all(HTML_HEAD.as_bytes()).await;
        let _ = socket.write_all(status_block.as_bytes()).await;
        let _ = socket.write_all(HTML_TAIL.as_bytes()).await;
        let _ = socket.flush().await;
        socket.close();
    }
}

fn extract_param<'a>(request: &'a str, key: &str) -> Option<&'a str> {
    let pos  = request.find(key)?;
    let after = &request[pos + key.len()..];
    let end  = after.find(|c: char| c == '&' || c == ' ' || c == '\r' || c == '\n')
                    .unwrap_or(after.len());
    Some(&after[..end])
}