#![no_std]
#![no_main]

extern crate alloc;
use embassy_net::{Config, Ipv4Address, Ipv4Cidr, Runner, Stack, StackResources, StaticConfigV4};
use embassy_time::{Duration, Timer};
use esp_backtrace as _;
use esp_hal::timer::timg::TimerGroup;
use esp_hal_dhcp_server::{
    simple_leaser::SimpleDhcpLeaser,
    structs::DhcpServerConfig,
    Ipv4Addr,
};
use esp_radio::wifi::{ap::AccessPointConfig, Config as WifiConfig, ControllerConfig, Interface, WifiController};

esp_bootloader_esp_idf::esp_app_desc!();

macro_rules! mk_static {
    ($t:ty,$val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write(($val));
        x
    }};
}

#[esp_hal::main]
async fn main(spawner: embassy_executor::Spawner) {
    esp_alloc::heap_allocator!(size: 150 * 1024);

    let peripherals = esp_hal::init(esp_hal::Config::default());

    esp_println::logger::init_logger_from_env();
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    let ap_config = WifiConfig::AccessPoint(
        AccessPointConfig::default().with_ssid("esp-radio".try_into().unwrap()),
    );
    let wifi_interface = Interface::access_point();
    let controller = WifiController::new(
        peripherals.WIFI,
        ControllerConfig::default().with_initial_config(ap_config),
    )
    .unwrap();
    log::info!("Wifi started!");

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

    spawner.spawn(connection(controller).unwrap());
    spawner.spawn(net_task(ap_runner).unwrap());

    loop {
        if stack.is_link_up() {
            break;
        }
        Timer::after(Duration::from_millis(500)).await;
    }

    log::info!("Connect to ap");
    spawner.spawn(dhcp_server(stack).unwrap());

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
        use_captive_portal: true,
    };

    // or `SingleDhcpLeaser::new(ip)` to always hand out one address
    let mut leaser = SimpleDhcpLeaser {
        start: Ipv4Addr::new(192, 168, 2, 50),
        end: Ipv4Addr::new(192, 168, 2, 200),
        leases: Default::default(),
    };

    let res = esp_hal_dhcp_server::run_dhcp_server(stack, config, &mut leaser).await;
    if let Err(e) = res {
        log::error!("DHCP SERVER ERROR: {e:?}");
    }
}

#[embassy_executor::task]
async fn connection(controller: WifiController<'static>) {
    log::info!("start connection task");
    loop {
        match controller.wait_for_access_point_connected_event_async().await {
            Ok(esp_radio::wifi::ap::EventInfo::Connected(info)) => {
                log::info!("Station connected: {info:?}")
            }
            Ok(esp_radio::wifi::ap::EventInfo::Disconnected(info)) => {
                log::info!("Station disconnected: {info:?}")
            }
            _ => (),
        }
    }
}

#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, Interface>) {
    runner.run().await
}
