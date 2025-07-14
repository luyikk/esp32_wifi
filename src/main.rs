use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::lazy_lock::LazyLock;

use embedded_svc::{
    http::{client::Client as HttpClient, Method},
    utils::io,
};
use esp_idf_svc::http::client::{Configuration, EspHttpConnection};

use embassy_time::Timer;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::peripherals::Peripherals;

use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::timer::EspTaskTimerService;
use esp_idf_svc::wifi::{
    AsyncWifi, ClientConfiguration as WifiClientConfig, Configuration as WifiConfig, EspWifi,
};

static CHANNEL: LazyLock<Channel<NoopRawMutex, u32, 1>> = LazyLock::new(|| Channel::new());

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    spawner.spawn(connect_wifi()).unwrap();

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
        let url = "https://www.google.com/";
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
async fn connect_wifi() {
    async fn init_wifi() -> anyhow::Result<()> {
        let peripherals = Peripherals::take()?;
        let sys_loop = EspSystemEventLoop::take()?;
        let timer_service = EspTaskTimerService::new()?;
        let nvs = EspDefaultNvsPartition::take()?;

        let mut wifi = AsyncWifi::wrap(
            EspWifi::new(peripherals.modem, sys_loop.clone(), Some(nvs))?,
            sys_loop,
            timer_service,
        )?;

        let ap_config = WifiConfig::Client(WifiClientConfig {
            ssid: "LY-TX".try_into().unwrap(),
            password: "a1234567890".try_into().unwrap(),
            ..Default::default()
        });

        wifi.set_configuration(&ap_config)?;
        wifi.start().await?;
        log::info!("Wifi started");
        loop {
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
                    log::warn!("WiFi disconnected, attempting to reconnect...");
                    break;
                }
                Timer::after(embassy_time::Duration::from_secs(5)).await;
            }
        }
    }

    if let Err(err) = init_wifi().await {
        log::error!("Failed to initialize peripherals: {:?}", err);
        return;
    }
}
