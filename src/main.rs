mod led;

use anyhow::Context;
use aqueue::Actor;
use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::lazy_lock::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};

use embedded_svc::{
    http::{client::Client as HttpClient, Method},
    utils::io,
};
use esp_idf_svc::http::client::{Configuration, EspHttpConnection};

use embassy_time::Timer;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::gpio::{Gpio2, PinDriver};
use esp_idf_svc::hal::peripherals::Peripherals;

use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs};

use esp_idf_svc::timer::EspTaskTimerService;
use esp_idf_svc::wifi::{
    AsyncWifi, ClientConfiguration as WifiClientConfig, Configuration as WifiConfig, EspWifi,
};
use once_cell::sync::OnceCell;

use crate::led::{ILed, Led};

static CHANNEL: LazyLock<Channel<NoopRawMutex, u32, 1>> = LazyLock::new(|| Channel::new());
static LED: OnceCell<Actor<Led<Gpio2>>> = OnceCell::new();
static LED_CHECK: AtomicBool = AtomicBool::new(false);

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    spawner.spawn(connect_wifi()).unwrap();
    spawner.spawn(led_check()).unwrap();

    log::info!("Hello, world!");
    loop {
        CHANNEL.get().receive().await;

        let mut client = HttpClient::wrap(
            EspHttpConnection::new(&Configuration {
                ..Default::default()
            })
            .unwrap(),
        );
        let headers = [("accept", "text/plain")];
        let url = "https://www.baidu.com/";
        let request = client.request(Method::Get, url, &headers).unwrap();
        log::info!("-> GET {url}");
        let mut response = request.submit().unwrap();
        // Process response
        let status = response.status();
        log::info!("<- {status}");
        let mut buf = vec![0u8; 4096];
        let bytes_read = io::try_read_full(&mut response, &mut buf)
            .map_err(|e| e.0)
            .unwrap();
        log::info!("Read {bytes_read} bytes");
        match std::str::from_utf8(&buf[0..bytes_read]) {
            Ok(body_string) => log::info!(
                "Response body (truncated to {} bytes): {:?}",
                buf.len(),
                body_string
            ),
            Err(e) => log::error!("Error decoding response body: {e}"),
        };

        Timer::after(embassy_time::Duration::from_secs(1)).await;
    }
}

#[embassy_executor::task]
async fn led_check() {
    async fn loop_loop() -> anyhow::Result<()> {
        loop {
            if LED.get().is_some() {
                if LED_CHECK.load(Ordering::Acquire) {
                    LED.get()
                        .unwrap()
                        .led2_on()
                        .await
                        .context("Failed to turn on LED")?;
                } else {
                    LED.get()
                        .unwrap()
                        .led2_off()
                        .await
                        .context("Failed to turn on LED")?;
                    Timer::after(embassy_time::Duration::from_millis(500)).await;
                    LED.get()
                        .unwrap()
                        .led2_on()
                        .await
                        .context("Failed to turn on LED")?;
                }
            }
            Timer::after(embassy_time::Duration::from_millis(500)).await;
        }
    }
    if let Err(err) = loop_loop().await {
        log::error!("Error in led_check: {:?}", err);
    }
}

#[embassy_executor::task]
async fn connect_wifi() {
    async fn init_wifi() -> anyhow::Result<()> {
        let peripherals = Peripherals::take()?;
        let sys_loop = EspSystemEventLoop::take()?;
        let timer_service = EspTaskTimerService::new()?;
        let nvs = EspDefaultNvsPartition::take()?;

        LED.set(Actor::new(Led::new(PinDriver::output(
            peripherals.pins.gpio2,
        )?)))
        .map_err(|_| anyhow::anyhow!("Failed to set LED actor"))?;

        let mut wifi = AsyncWifi::wrap(
            EspWifi::new(peripherals.modem, sys_loop.clone(), Some(nvs.clone()))?,
            sys_loop,
            timer_service,
        )?;

        let mut ns = EspNvs::new(nvs, "config", true)?;

        if !ns.contains("ssid")? {
            ns.set_str("ssid", "LY-TX")?;
        }
        let len = ns
            .str_len("ssid")?
            .context("Failed to get password length from NVS")?;
        let mut _r = vec![0u8; len];
        let ssid = ns
            .get_str("ssid", &mut _r)?
            .context("Failed to get ssid from NVS")?;
        log::info!("从 NVS 获取的ssid: {ssid}");

        if !ns.contains("password")? {
            ns.set_str("password", "a1234567890")?;
        }
        let len = ns
            .str_len("password")?
            .context("Failed to get password length from NVS")?;
        let mut _r = vec![0u8; len];
        let password = ns
            .get_str("password", &mut _r)?
            .context("Failed to get password from NVS")?;
        log::info!("从 NVS 获取的密码: {password}");

        let ap_config = WifiConfig::Client(WifiClientConfig {
            ssid: ssid.try_into().unwrap(),
            password: password.try_into().unwrap(),
            ..Default::default()
        });

        wifi.set_configuration(&ap_config)?;
        wifi.start().await?;
        log::info!("Wifi started");
        loop {
            LED.get()
                .context("LED actor not initialized")?
                .led2_off()
                .await
                .context("Failed to turn on LED")?;

            let aps = wifi.wifi_mut().scan()?;
            for ap in aps {
                log::info!("可用 WiFi SSID: {}", ap.ssid);
            }

            if let Err(err) = wifi.connect().await {
                log::error!("Failed to connect to WiFi: {:?}", err);
                Timer::after(embassy_time::Duration::from_secs(5)).await;
                continue;
            }

            log::info!("Wifi connected");

            wifi.wait_netif_up().await?;
            log::info!("Wifi netif up");

            let ip_info = wifi.wifi().sta_netif().get_ip_info()?;
            log::info!("Wifi DHCP info: {ip_info:?}");

            log::info!("WiFi initialized successfully");

            CHANNEL.get().send(1).await;

            loop {
                if let Ok(false) = wifi.wifi().is_connected() {
                    LED.get()
                        .context("LED actor not initialized")?
                        .led2_off()
                        .await
                        .context("Failed to turn on LED")?;
                    LED_CHECK.store(false, Ordering::Release);
                    log::warn!("WiFi disconnected, attempting to reconnect...");
                    break;
                }

                LED_CHECK.store(true, Ordering::Release);
                Timer::after(embassy_time::Duration::from_secs(1)).await;
            }
        }
    }

    if let Err(err) = init_wifi().await {
        log::error!("Failed to initialize peripherals: {:?}", err);
        return;
    }
}
