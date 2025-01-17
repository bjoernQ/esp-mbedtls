//! Example for an async server.
//! Contains a basic server implementation to test mbedtls in server mode.
//!
//! This example uses self-signed certificate. Your browser may display an error.
//! You have to enable the exception to then proceed, of if using curl, use the flag `-k`.
#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]
#![feature(impl_trait_in_assoc_type)]

#[doc(hidden)]
pub use esp_hal as hal;

use embassy_net::tcp::TcpSocket;
use embassy_net::{Config, IpListenEndpoint, Runner, StackResources};

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_alloc as _;
use esp_backtrace as _;
use esp_mbedtls::{asynch::Session, Certificates, Mode, TlsVersion};
use esp_mbedtls::{Tls, TlsError, X509};
use esp_println::logger::init_logger;
use esp_println::{print, println};
use esp_wifi::wifi::{
    ClientConfiguration, Configuration, WifiController, WifiDevice, WifiEvent, WifiStaDevice,
    WifiState,
};
use esp_wifi::{init, EspWifiController};
use hal::{clock::CpuClock, rng::Rng, timer::timg::TimerGroup};

// Patch until https://github.com/embassy-rs/static-cell/issues/16 is fixed
macro_rules! mk_static {
    ($t:ty,$val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write(($val));
        x
    }};
}

const SSID: &str = env!("SSID");
const PASSWORD: &str = env!("PASSWORD");

#[esp_hal_embassy::main]
async fn main(spawner: Spawner) -> ! {
    init_logger(log::LevelFilter::Info);

    let peripherals = esp_hal::init({
        let mut config = esp_hal::Config::default();
        config.cpu_clock = CpuClock::max();
        config
    });

    esp_alloc::heap_allocator!(115 * 1024);

    let timg0 = TimerGroup::new(peripherals.TIMG0);

    let init = &*mk_static!(
        EspWifiController<'_>,
        init(
            timg0.timer0,
            Rng::new(peripherals.RNG),
            peripherals.RADIO_CLK,
        )
        .unwrap()
    );

    let wifi = peripherals.WIFI;
    let (wifi_interface, controller) =
        esp_wifi::wifi::new_with_mode(init, wifi, WifiStaDevice).unwrap();

    cfg_if::cfg_if! {
        if #[cfg(feature = "esp32")] {
            let timg1 = TimerGroup::new(peripherals.TIMG1);
            esp_hal_embassy::init(timg1.timer0);
        } else {
            use esp_hal::timer::systimer::SystemTimer;
            let systimer = SystemTimer::new(peripherals.SYSTIMER);
            esp_hal_embassy::init(systimer.alarm0);
        }
    }

    let config = Config::dhcpv4(Default::default());

    let seed = 1234; // very random, very secure seed

    // Init network stack
    let (stack, runner) = embassy_net::new(
        wifi_interface,
        config,
        mk_static!(StackResources<3>, StackResources::<3>::new()),
        seed,
    );

    spawner.spawn(connection(controller)).ok();
    spawner.spawn(net_task(runner)).ok();

    let mut tls = Tls::new(peripherals.SHA)
        .unwrap()
        .with_hardware_rsa(peripherals.RSA);

    tls.set_debug(0);

    let mut rx_buffer = [0; 4096];
    let mut tx_buffer = [0; 4096];

    loop {
        if stack.is_link_up() {
            break;
        }
        Timer::after(Duration::from_millis(500)).await;
    }

    println!("Waiting to get IP address...");
    loop {
        if let Some(config) = stack.config_v4() {
            println!("Got IP: {}", config.address);
            println!(
                "Point your browser to https://{}/",
                config.address.address()
            );
            break;
        }
        Timer::after(Duration::from_millis(500)).await;
    }

    let mut socket = TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);
    socket.set_timeout(Some(Duration::from_secs(10)));
    loop {
        println!("Waiting for connection...");
        let r = socket
            .accept(IpListenEndpoint {
                addr: None,
                port: 443,
            })
            .await;
        println!("Connected...");

        if let Err(e) = r {
            println!("connect error: {:?}", e);
            continue;
        }

        use embedded_io_async::Write;

        let mut buffer = [0u8; 1024];
        let mut pos = 0;
        let mut session = Session::new(
            &mut socket,
            Mode::Server,
            TlsVersion::Tls1_2,
            Certificates {
                // Use self-signed certificates
                certificate: X509::pem(
                    concat!(include_str!("./certs/certificate.pem"), "\0").as_bytes(),
                )
                .ok(),
                private_key: X509::pem(
                    concat!(include_str!("./certs/private_key.pem"), "\0").as_bytes(),
                )
                .ok(),
                ..Default::default()
            },
            tls.reference(),
        )
        .unwrap();

        println!("Start tls connect");
        match session.connect().await {
            Ok(()) => {
                log::info!("Got session");
                loop {
                    match session.read(&mut buffer).await {
                        Ok(0) => {
                            println!("read EOF");
                            break;
                        }
                        Ok(len) => {
                            let to_print =
                                unsafe { core::str::from_utf8_unchecked(&buffer[..(pos + len)]) };

                            if to_print.contains("\r\n\r\n") {
                                print!("{}", to_print);
                                println!();
                                break;
                            }

                            pos += len;
                        }
                        Err(e) => {
                            println!("read error: {:?}", e);
                            break;
                        }
                    };
                }

                let r = session
                    .write_all(
                        b"HTTP/1.0 200 OK\r\n\r\n\
                            <html>\
                                <body>\
                                    <h1>Hello Rust! Hello esp-mbedtls!</h1>\
                                </body>\
                            </html>\r\n\
                            ",
                    )
                    .await;
                if let Err(e) = r {
                    println!("write error: {:?}", e);
                }

                Timer::after(Duration::from_millis(1000)).await;
            }
            Err(TlsError::MbedTlsError(-30592)) => {
                println!("Fatal message: Please enable the exception for a self-signed certificate in your browser");
            }
            Err(error) => {
                panic!("{:?}", error);
            }
        }
        drop(session);
        println!("Closing socket");
        socket.close();
        Timer::after(Duration::from_millis(1000)).await;

        socket.abort();
    }
}

#[embassy_executor::task]
async fn connection(mut controller: WifiController<'static>) {
    println!("start connection task");
    println!("Device capabilities: {:?}", controller.capabilities());
    loop {
        if matches!(esp_wifi::wifi::wifi_state(), WifiState::StaConnected) {
            // wait until we're no longer connected
            controller.wait_for_event(WifiEvent::StaDisconnected).await;
            Timer::after(Duration::from_millis(5000)).await
        }
        if !matches!(controller.is_started(), Ok(true)) {
            let client_config = Configuration::Client(ClientConfiguration {
                ssid: SSID.try_into().unwrap(),
                password: PASSWORD.try_into().unwrap(),
                ..Default::default()
            });
            controller.set_configuration(&client_config).unwrap();
            println!("Starting wifi");
            controller.start_async().await.unwrap();
            println!("Wifi started!");
        }
        println!("About to connect...");

        match controller.connect_async().await {
            Ok(_) => println!("Wifi connected!"),
            Err(e) => {
                println!("Failed to connect to wifi: {e:?}");
                Timer::after(Duration::from_millis(5000)).await
            }
        }
    }
}

#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, WifiDevice<'static, WifiStaDevice>>) {
    runner.run().await
}
