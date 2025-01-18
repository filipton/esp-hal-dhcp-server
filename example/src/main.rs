#![no_std]
#![no_main]

extern crate alloc;
use embassy_net::{Config, Ipv4Address, Ipv4Cidr, Runner, Stack, StackResources, StaticConfigV4};
use embassy_time::{Duration, Timer};
use esp_backtrace as _;
use esp_hal::timer::timg::TimerGroup;
use esp_hal_dhcp_server::{
    simple_leaser::{SimpleDhcpLeaser, SingleDhcpLeaser},
    structs::DhcpServerConfig,
    Ipv4Addr,
};
use esp_hal_embassy::main;
use esp_wifi::{
    wifi::{
        AccessPointConfiguration, Configuration, WifiApDevice, WifiController, WifiDevice,
        WifiEvent, WifiState,
    },
    EspWifiController,
};

macro_rules! mk_static {
    ($t:ty,$val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write(($val));
        x
    }};
}

#[main]
async fn main(spawner: embassy_executor::Spawner) {
    esp_alloc::heap_allocator!(150 * 1024);

    let peripherals = esp_hal::init(esp_hal::Config::default());

    esp_println::logger::init_logger_from_env();
    let timg1 = TimerGroup::new(peripherals.TIMG1);
    esp_hal_embassy::init(timg1.timer0);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let init = esp_wifi::init(
        timg0.timer0,
        esp_hal::rng::Rng::new(peripherals.RNG),
        peripherals.RADIO_CLK,
    )
    .unwrap();
    let init = &*mk_static!(EspWifiController<'static>, init);

    let wifi = peripherals.WIFI;
    let (wifi_interface, controller) =
        esp_wifi::wifi::new_with_mode(&init, wifi, WifiApDevice).unwrap();

    let config = Config::ipv4_static(StaticConfigV4 {
        address: Ipv4Cidr::new(Ipv4Address::new(192, 168, 2, 1), 24),
        gateway: Some(Ipv4Address::new(192, 168, 2, 1)),
        dns_servers: Default::default(),
    });
    let seed = 1234; // very random, very secure seed

    // Init network stack
    let (stack, ap_runner) = embassy_net::new(
        wifi_interface,
        config,
        mk_static!(StackResources<3>, StackResources::<3>::new()),
        seed,
    );

    spawner.spawn(connection(controller)).ok();
    spawner.spawn(net_task(ap_runner)).ok();

    loop {
        if stack.is_link_up() {
            break;
        }
        Timer::after(Duration::from_millis(500)).await;
    }

    log::info!("Connect to ap");
    spawner.spawn(dhcp_server(stack)).ok();

    Timer::after(Duration::from_secs(120)).await;
    log::info!("Closing dhcp server after 2m...");
    esp_hal_dhcp_server::dhcp_close();
}

#[embassy_executor::task]
async fn dhcp_server(stack: Stack<'static>) {
    let config = DhcpServerConfig {
        ip: Ipv4Addr::new(192, 168, 2, 1),
        lease_time: Duration::from_secs(3600),
        gateways: &[Ipv4Addr::new(192, 168, 2, 1)],
        subnet: None,
        dns: &[Ipv4Addr::new(192, 168, 2, 1)],
    };

    /*
    let mut leaser = SimpleDhcpLeaser {
        start: Ipv4Addr::new(192, 168, 2, 50),
        end: Ipv4Addr::new(192, 168, 2, 200),
        leases: Default::default(),
    };
    */
    let mut leaser = SingleDhcpLeaser::new(Ipv4Addr::new(192, 168, 2, 69));

    let res = esp_hal_dhcp_server::run_dhcp_server(stack, config, &mut leaser).await;
    if let Err(e) = res {
        log::error!("DHCP SERVER ERROR: {e:?}");
    }
}

#[embassy_executor::task]
async fn connection(mut controller: WifiController<'static>) {
    log::info!("start connection task");
    log::info!("Device capabilities: {:?}", controller.capabilities());
    loop {
        match esp_wifi::wifi::wifi_state() {
            WifiState::ApStarted => {
                // wait until we're no longer connected
                controller.wait_for_event(WifiEvent::ApStop).await;
                Timer::after(Duration::from_millis(5000)).await
            }
            _ => {}
        }
        if !matches!(controller.is_started(), Ok(true)) {
            let client_config = Configuration::AccessPoint(AccessPointConfiguration {
                ssid: "esp-wifi".try_into().unwrap(),
                ..Default::default()
            });
            controller.set_configuration(&client_config).unwrap();
            log::info!("Starting wifi");
            controller.start_async().await.unwrap();
            log::info!("Wifi started!");
        }
    }
}

#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, WifiDevice<'static, WifiApDevice>>) {
    runner.run().await
}
