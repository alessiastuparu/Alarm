#![no_std]
#![no_main]

use core::fmt::Write as FmtWrite;
use embedded_hal::blocking::serial::Write as BlockingWrite;
use cyw43_pio::{PioSpi, RM2_CLOCK_DIVIDER};
use defmt::info;
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_net::{Config, StackResources};
use embassy_rp::bind_interrupts;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{DMA_CH0, PIO0};
use embassy_rp::pio::{InterruptHandler, Pio};
use embassy_rp::uart::Async;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::{Duration, Timer};
use heapless::String;
use panic_probe as _;
use static_cell::StaticCell;

const AP_SSID: &str = "SunriseAlarm";

struct Telemetry {
    temp_tenths: i16,
    alarm_hour: u8,
    alarm_minute: u8,
    alarm_enabled: bool,
    alarm_ringing: bool,
    cur_hour: u8,
    cur_minute: u8,
    cur_second: u8,
    fresh: bool,
}

static TELEMETRY: Mutex<CriticalSectionRawMutex, Telemetry> = Mutex::new(Telemetry {
    temp_tenths: 0,
    alarm_hour: 0,
    alarm_minute: 0,
    alarm_enabled: false,
    alarm_ringing: false,
    cur_hour: 0,
    cur_minute: 0,
    cur_second: 0,
    fresh: false,
});

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
async fn uart_reader(mut rx: embassy_rp::uart::UartRx<'static, Async>) {
    let mut buf = [0u8; 1];
    let mut line: String<128> = String::new();
    info!("UART reader started — waiting for STM32 telemetry...");

    loop {
        match rx.read(&mut buf).await {
            Ok(_) => {
                let b = buf[0];
                if b == b'\n' {
                    if !line.is_empty() {
                        parse_telemetry(line.as_str()).await;
                    }
                    line.clear();
                } else if b != b'\r' {
                    let _ = line.push(b as char);
                }
            }
            Err(_) => {
                Timer::after(Duration::from_millis(10)).await;
            }
        }
    }
}

async fn parse_telemetry(line: &str) {
    let mut temp_tenths: i16 = 0;
    let mut alarm_hour: u8 = 0;
    let mut alarm_minute: u8 = 0;
    let mut alarm_enabled: bool = false;
    let mut alarm_ringing: bool = false;
    let mut cur_hour: u8 = 0;
    let mut cur_minute: u8 = 0;
    let mut cur_second: u8 = 0;

    for part in line.split(',') {
        if let Some(val) = part.strip_prefix("T:") {
            temp_tenths = val.trim().parse::<i16>().unwrap_or(0);
        } else if let Some(val) = part.strip_prefix("AH:") {
            alarm_hour = val.trim().parse::<u8>().unwrap_or(0);
        } else if let Some(val) = part.strip_prefix("AM:") {
            alarm_minute = val.trim().parse::<u8>().unwrap_or(0);
        } else if let Some(val) = part.strip_prefix("AE:") {
            alarm_enabled = val.trim() == "1";
        } else if let Some(val) = part.strip_prefix("AR:") {
            alarm_ringing = val.trim() == "1";
        } else if let Some(val) = part.strip_prefix("CH:") {
            cur_hour = val.trim().parse::<u8>().unwrap_or(0);
        } else if let Some(val) = part.strip_prefix("CM:") {
            cur_minute = val.trim().parse::<u8>().unwrap_or(0);
        } else if let Some(val) = part.strip_prefix("CS:") {
            cur_second = val.trim().parse::<u8>().unwrap_or(0);
        }
    }

    info!("Telemetry parsed: time {:02}:{:02}:{:02} temp={}",
        cur_hour, cur_minute, cur_second, temp_tenths);

    let mut telem = TELEMETRY.lock().await;
    telem.temp_tenths = temp_tenths;
    telem.alarm_hour = alarm_hour;
    telem.alarm_minute = alarm_minute;
    telem.alarm_enabled = alarm_enabled;
    telem.alarm_ringing = alarm_ringing;
    telem.cur_hour = cur_hour;
    telem.cur_minute = cur_minute;
    telem.cur_second = cur_second;
    telem.fresh = true;
}

const HTML_HEAD: &str = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n\
<!DOCTYPE html><html><head>\
<meta charset='UTF-8'><meta name='viewport' content='width=device-width,initial-scale=1.0'>\
<meta http-equiv='refresh' content='5'><title>Sunrise Alarm</title>\
<style>\
body{font-family:sans-serif;text-align:center;margin-top:50px;background:#f4f4f9;color:#333}\
.card{background:white;padding:30px;border-radius:10px;display:inline-block;\
box-shadow:0 4px 8px rgba(0,0,0,.1);min-width:320px}\
.status{background:#e8f5e9;padding:15px;border-radius:8px;margin:15px 0;text-align:left}\
.status p{margin:6px 0;font-size:15px}\
.ringing{background:#ffebee;animation:pulse 1s infinite}\
@keyframes pulse{0%{opacity:1}50%{opacity:.5}100%{opacity:1}}\
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
<h1>&#127749; Sunrise Alarm</h1><div class='status ";

const HTML_TAIL: &str = "</div>\
<hr><h3>Alarm Settings</h3>\
<input type='time' id='t'><br>\
<button class='gs' onclick='setAlarm()'>Set Alarm</button>\
<button class='ds' onclick=\"fetch('/disable').then(()=>location.reload())\">Disable</button>\
<hr><h3>Actions</h3>\
<button class='sn' onclick=\"fetch('/snooze').then(()=>location.reload())\">Snooze 5min</button>\
<button class='sy' onclick='syncTime()'>Sync Time</button>\
<p class='note'>Auto-refreshes every 5s</p></div>\
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

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());
    info!("Pico 2W starting in AP mode...");

    let mut uart_config = embassy_rp::uart::Config::default();
    uart_config.baudrate = 115200;

    let uart = embassy_rp::uart::Uart::new(
        p.UART0, p.PIN_0, p.PIN_1,
        Irqs, p.DMA_CH1, p.DMA_CH2,
        uart_config,
    );
    let (mut uart_tx, uart_rx) = uart.split();

    let fw  = include_bytes!("../firmware/43439A0.bin");
    let clm = include_bytes!("../firmware/43439A0_clm.bin");

    let pwr = Output::new(p.PIN_23, Level::Low);
    let cs  = Output::new(p.PIN_25, Level::High);
    let mut pio = Pio::new(p.PIO0, Irqs);

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

    info!("Starting open AP '{}'...", AP_SSID);
    control.start_ap_open(AP_SSID, 6).await;
    info!("AP up! Connect to '{}' then open http://192.168.4.1", AP_SSID);

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

    spawner.spawn(uart_reader(uart_rx).unwrap());

    Timer::after(Duration::from_secs(2)).await;
    info!("HTTP server ready at http://192.168.4.1");

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

        if request.starts_with("GET /snooze") {
            let _ = uart_tx.bwrite_all(b"SN\n");
            info!("Sent SN to STM32");

        } else if request.starts_with("GET /disable") {
            let _ = uart_tx.bwrite_all(b"DA\n");
            info!("Sent DA to STM32");

        } else if request.starts_with("GET /set_alarm") {
            if let (Some(h), Some(m)) = (
                extract_param(request, "h="),
                extract_param(request, "m="),
            ) {
                let mut msg: String<16> = String::new();
                core::write!(msg, "SA:{}:{}\n", h, m).ok();
                let _ = uart_tx.bwrite_all(msg.as_bytes());
                info!("Sent SA:{}:{} to STM32", h, m);
            }

        } else if request.starts_with("GET /sync_time") {
            if let (Some(h), Some(m), Some(s)) = (
                extract_param(request, "h="),
                extract_param(request, "m="),
                extract_param(request, "s="),
            ) {
                let mut msg: String<20> = String::new();
                core::write!(msg, "ST:{}:{}:{}\n", h, m, s).ok();
                let _ = uart_tx.bwrite_all(msg.as_bytes());
                info!("Sent ST:{}:{}:{} to STM32", h, m, s);
            }
        }

        let (status_html, is_ringing) = {
            let telem = TELEMETRY.lock().await;
            let mut s: String<512> = String::new();
            let ringing = telem.alarm_ringing;

            if telem.fresh {
                let temp_whole = telem.temp_tenths / 10;
                let temp_frac  = (telem.temp_tenths % 10).unsigned_abs();
                core::write!(
                    s,
                    "<p>&#128336; <b>Time:</b> {:02}:{:02}:{:02}</p>\
                     <p>&#9200; <b>Alarm:</b> {:02}:{:02} &mdash; {}</p>\
                     <p>&#127777; <b>Temp:</b> {}.{}&#176;C</p>{}",
                    telem.cur_hour, telem.cur_minute, telem.cur_second,
                    telem.alarm_hour, telem.alarm_minute,
                    if telem.alarm_enabled { "ON &#9989;" } else { "OFF" },
                    temp_whole, temp_frac,
                    if ringing { "<p><b>&#128276; ALARM RINGING!</b></p>" } else { "" },
                ).ok();
            } else {
                core::write!(s,
                    "<p class='wait'>&#9201; Waiting for STM32 telemetry...</p>"
                ).ok();
            }
            (s, ringing)
        };
        let ringing_class = if is_ringing { "ringing'" } else { "'" };
        let _ = socket.write(HTML_HEAD.as_bytes()).await;
        let _ = socket.write(ringing_class.as_bytes()).await;
        let _ = socket.write(b">").await;
        let _ = socket.write(status_html.as_bytes()).await;
        let _ = socket.write(HTML_TAIL.as_bytes()).await;
        let _ = socket.flush().await;
        socket.close();
    }
}

fn extract_param<'a>(req: &'a str, key: &str) -> Option<&'a str> {
    let pos = req.find(key)?;
    let after = &req[pos + key.len()..];
    let end = after
        .find(|c: char| c == '&' || c == ' ' || c == '\r' || c == '\n')
        .unwrap_or(after.len());
    Some(&after[..end])
}