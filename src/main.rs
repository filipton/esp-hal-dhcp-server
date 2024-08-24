#![no_std]
#![no_main]

use edge_dhcp::{server::Action, DhcpOption, Ipv4Addr, MessageType, Options, Packet};
use embassy_net::{
    udp::{PacketMetadata, UdpSocket},
    Config, Ipv4Address, Ipv4Cidr, Stack, StackResources, StaticConfigV4,
};
use embassy_time::{Duration, Timer};
use esp_backtrace as _;
use esp_hal::{
    clock::ClockControl,
    delay::Delay,
    peripherals::Peripherals,
    prelude::*,
    system::SystemControl,
    timer::{timg::TimerGroup, ErasedTimer, OneShotTimer},
};
use esp_wifi::{
    initialize,
    wifi::{
        AccessPointConfiguration, Configuration, WifiApDevice, WifiController, WifiDevice,
        WifiEvent, WifiState,
    },
    EspWifiInitFor,
};

macro_rules! mk_static {
    ($t:ty,$val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write(($val));
        x
    }};
}

const DHCP_BROADCAST: embassy_net::IpEndpoint =
    embassy_net::IpEndpoint::new(embassy_net::IpAddress::v4(255, 255, 255, 255), 68);

#[main]
async fn main(spawner: embassy_executor::Spawner) {
    let peripherals = Peripherals::take();
    let system = SystemControl::new(peripherals.SYSTEM);

    let clocks = ClockControl::max(system.clock_control).freeze();

    esp_println::logger::init_logger_from_env();
    let timg1 = TimerGroup::new(peripherals.TIMG1, &clocks, None);
    let timer0 = OneShotTimer::new(timg1.timer0.into());
    let timers = [timer0];
    let timers: &mut [OneShotTimer<ErasedTimer>; 1] =
        mk_static!([OneShotTimer<ErasedTimer>; 1], timers);
    esp_hal_embassy::init(&clocks, timers);

    let timer = esp_hal::timer::PeriodicTimer::new(
        esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0, &clocks, None)
            .timer0
            .into(),
    );

    let init = initialize(
        EspWifiInitFor::Wifi,
        timer,
        esp_hal::rng::Rng::new(peripherals.RNG),
        peripherals.RADIO_CLK,
        &clocks,
    )
    .unwrap();

    let wifi = peripherals.WIFI;
    let (wifi_interface, controller) =
        esp_wifi::wifi::new_with_mode(&init, wifi, WifiApDevice).unwrap();

    let config = Config::ipv4_static(StaticConfigV4 {
        address: Ipv4Cidr::new(Ipv4Address::new(192, 168, 2, 1), 24),
        gateway: Some(Ipv4Address::from_bytes(&[192, 168, 2, 1])),
        dns_servers: Default::default(),
    });
    let seed = 1234; // very random, very secure seed

    // Init network stack
    let stack = &*mk_static!(
        Stack<WifiDevice<'_, WifiApDevice>>,
        Stack::new(
            wifi_interface,
            config,
            mk_static!(StackResources<3>, StackResources::<3>::new()),
            seed
        )
    );

    spawner.spawn(connection(controller)).ok();
    spawner.spawn(net_task(&stack)).ok();

    loop {
        if stack.is_link_up() {
            break;
        }
        Timer::after(Duration::from_millis(500)).await;
    }

    log::info!("Connect to ap");

    let mut rx_buffer = [0; 1024];
    let mut tx_buffer = [0; 1024];
    let mut rx_meta = [PacketMetadata::EMPTY; 16];
    let mut tx_meta = [PacketMetadata::EMPTY; 16];
    let mut sock = UdpSocket::new(
        &stack,
        &mut rx_meta,
        &mut rx_buffer,
        &mut tx_meta,
        &mut tx_buffer,
    );

    let endpoint = embassy_net::IpEndpoint::new(embassy_net::IpAddress::v4(0, 0, 0, 0), 67);
    sock.bind(endpoint).unwrap();

    // ensure same as ap ip
    let server_ip = Ipv4Addr::new(192, 168, 2, 1);

    let mut buf = [0; 1024];
    loop {
        let res = sock.recv_from(&mut buf).await;
        if let Ok((n, addr)) = res {
            log::info!("received {n} from {addr:?}");

            let res = Packet::decode(&buf[..n]);
            if let Ok(packet) = res {
                let action = process_packet(&packet, server_ip).unwrap();

                let mut opt_buf = Options::buf();
                let reply_packet = match action {
                    Action::Discover(_requested_ip, _mac) => {
                        // TODO: make this into trait function (to get ip from mac, and considering requested_ip)
                        let ip = Ipv4Addr::new(192, 168, 2, 69);

                        let reply = packet.new_reply(
                            Some(ip),
                            packet.options.reply(
                                edge_dhcp::MessageType::Offer,
                                server_ip,
                                3600,
                                &[],
                                None,
                                &[],
                                &mut opt_buf,
                            ),
                        );
                        Some(reply)
                    }
                    Action::Request(ip, _mac) => {
                        // TODO: make this into trait function as well
                        let ip = Some(ip); // return none if NAK

                        let msg_type = match ip {
                            Some(_) => MessageType::Ack,
                            None => MessageType::Nak,
                        };

                        let reply = packet.new_reply(
                            ip,
                            packet.options.reply(
                                msg_type,
                                server_ip,
                                3600,
                                &[],
                                None,
                                &[],
                                &mut opt_buf,
                            ),
                        );
                        Some(reply)
                    }
                    Action::Release(_ip, _mac) | Action::Decline(_ip, _mac) => {
                        // TODO: remove lease

                        None
                    }
                };

                if let Some(reply) = reply_packet {
                    let bytes_res = reply.encode(&mut buf);
                    match bytes_res {
                        Ok(bytes) => {
                            let res = sock.send_to(bytes, DHCP_BROADCAST).await;
                            if let Err(e) = res {
                                log::error!("Dhcp sock send error: {e:?}");
                            }
                        }
                        Err(e) => {
                            log::error!("Dhcp encode error: {e:?}");
                        }
                    }
                }
            }
        }
    }
}
#[embassy_executor::task]
async fn connection(mut controller: WifiController<'static>) {
    log::info!("start connection task");
    log::info!("Device capabilities: {:?}", controller.get_capabilities());
    loop {
        match esp_wifi::wifi::get_wifi_state() {
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
            controller.start().await.unwrap();
            log::info!("Wifi started!");
        }
    }
}

#[embassy_executor::task]
async fn net_task(stack: &'static Stack<WifiDevice<'static, WifiApDevice>>) {
    stack.run().await
}

pub fn process_packet<'a>(request: &'a Packet<'a>, server_ip: Ipv4Addr) -> Option<Action<'a>> {
    if request.reply {
        return None;
    }

    let message_type = request.options.iter().find_map(|option| match option {
        DhcpOption::MessageType(msg_type) => Some(msg_type),
        _ => None,
    });

    let message_type = message_type.or_else(|| {
        log::warn!("Ignoring DHCP request, no message type found: {request:?}");
        None
    })?;

    let server_identifier = request.options.iter().find_map(|option| match option {
        DhcpOption::ServerIdentifier(ip) => Some(ip),
        _ => None,
    });

    if server_identifier.is_some() && server_identifier != Some(server_ip) {
        log::warn!("Ignoring {message_type} request, not addressed to this server: {request:?}");
        return None;
    }

    match message_type {
        MessageType::Discover => Some(Action::Discover(
            requested_ip(&request.options),
            &request.chaddr,
        )),
        MessageType::Request => {
            let requested_ip = requested_ip(&request.options).or_else(|| {
                match request.ciaddr.is_unspecified() {
                    true => None,
                    false => Some(request.ciaddr),
                }
            })?;

            Some(Action::Request(requested_ip, &request.chaddr))
        }
        MessageType::Release if server_identifier == Some(server_ip) => {
            Some(Action::Release(request.yiaddr, &request.chaddr))
        }
        MessageType::Decline if server_identifier == Some(server_ip) => {
            Some(Action::Decline(request.yiaddr, &request.chaddr))
        }
        _ => None,
    }
}

pub fn requested_ip<'a>(options: &'a Options<'a>) -> Option<Ipv4Addr> {
    options.iter().find_map(|option| {
        if let DhcpOption::RequestedIpAddress(ip) = option {
            Some(ip)
        } else {
            None
        }
    })
}
