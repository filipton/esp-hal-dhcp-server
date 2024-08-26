#![no_std]
#![no_main]

use edge_dhcp::{server::Action, DhcpOption, Ipv4Addr, MessageType, Options, Packet};
use embassy_net::{
    udp::{PacketMetadata, UdpSocket},
    Config, Ipv4Address, Ipv4Cidr, Stack, StackResources, StaticConfigV4,
};
use embassy_time::{Duration, Instant, Timer};
use esp_backtrace as _;
use esp_hal::{
    clock::ClockControl,
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
    let sock = UdpSocket::new(
        &stack,
        &mut rx_meta,
        &mut rx_buffer,
        &mut tx_meta,
        &mut tx_buffer,
    );

    let mut test_dhcp = TestDhcpLeaser {
        leases: heapless::Vec::new(),
        start: Ipv4Addr::new(192, 168, 2, 10),
        end: Ipv4Addr::new(192, 168, 2, 20),
    };

    let mut server = DhcpServer::new(
        Ipv4Addr::new(192, 168, 2, 1),
        Duration::from_secs(3600),
        &mut test_dhcp,
        sock,
    );
    server.run().await;
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

#[derive(Debug, Clone)]
pub struct DhcpLease {
    pub ip: Ipv4Addr,
    pub mac: [u8; 16],
    pub expires: Instant,
}

pub trait DhcpLeaser {
    fn get_lease(&mut self, mac: [u8; 16]) -> Option<DhcpLease>;
    fn next_lease(&mut self) -> Option<Ipv4Addr>;
    fn add_lease(&mut self, ip: Ipv4Addr, mac: [u8; 16], expires: Instant) -> bool;
    fn remove_lease(&mut self, mac: [u8; 16]) -> bool;
}

pub struct TestDhcpLeaser {
    pub start: Ipv4Addr,
    pub end: Ipv4Addr,
    pub leases: heapless::Vec<DhcpLease, 16>,
}

impl DhcpLeaser for TestDhcpLeaser {
    fn get_lease(&mut self, mac: [u8; 16]) -> Option<DhcpLease> {
        for lease in &self.leases {
            if lease.mac == mac {
                return Some(lease.clone());
            }
        }

        None
    }

    fn next_lease(&mut self) -> Option<Ipv4Addr> {
        let start: u32 = self.start.into();
        let end: u32 = self.end.into();

        for ip in start..=end {
            let ip: Ipv4Addr = ip.into();
            let mut found = false;

            for lease in &self.leases {
                if lease.ip == ip {
                    found = true;
                }
            }

            if !found {
                return Some(ip);
            }
        }

        None
    }

    fn add_lease(&mut self, ip: Ipv4Addr, mac: [u8; 16], expires: Instant) -> bool {
        self.remove_lease(mac);
        self.leases.push(DhcpLease { ip, mac, expires }).is_ok()
    }

    fn remove_lease(&mut self, mac: [u8; 16]) -> bool {
        for (i, lease) in self.leases.iter().enumerate() {
            if lease.mac == mac {
                self.leases.remove(i);
                return true;
            }
        }

        false
    }
}

const DHCP_BROADCAST: embassy_net::IpEndpoint =
    embassy_net::IpEndpoint::new(embassy_net::IpAddress::v4(255, 255, 255, 255), 68);
const DHCP_SERVER_ENDPOINT: embassy_net::IpEndpoint =
    embassy_net::IpEndpoint::new(embassy_net::IpAddress::v4(0, 0, 0, 0), 67);
const DHCP_BUFFER_SIZE: usize = 1024;

pub struct DhcpServer<'a> {
    ip: Ipv4Addr,
    lease_time: Duration,

    leaser: &'a mut dyn DhcpLeaser,
    sock: UdpSocket<'a>,
}

impl<'a> DhcpServer<'a> {
    pub fn new(
        ip: Ipv4Addr,
        lease_time: Duration,
        leaser: &'a mut dyn DhcpLeaser,
        mut sock: UdpSocket<'a>,
    ) -> Self {
        sock.bind(DHCP_SERVER_ENDPOINT).unwrap();

        Self {
            ip,
            lease_time,
            leaser,
            sock,
        }
    }

    pub async fn run(&mut self) {
        let mut buf = [0; DHCP_BUFFER_SIZE];
        loop {
            let res = self.sock.recv_from(&mut buf).await;
            if let Ok((n, addr)) = res {
                log::info!("received {n} from {addr:?}");

                let res = Packet::decode(&buf[..n]);
                if let Ok(packet) = res {
                    self.process_packet(packet).await;
                }
            }
        }
    }

    async fn process_packet(&mut self, packet: Packet<'_>) {
        let action = self.get_packet_action(&packet).unwrap();

        match action {
            Action::Discover(requested_ip, mac) => {
                let ip = requested_ip
                    .and_then(|ip| {
                        let mac_lease = self.leaser.get_lease(*mac);
                        let available = mac_lease
                            .map(|d| d.ip == ip || Instant::now() > d.expires)
                            .unwrap_or(true);

                        available.then_some(ip)
                    })
                    .or_else(|| self.leaser.get_lease(*mac).map(|l| l.ip))
                    .or_else(|| self.leaser.next_lease());

                if ip.is_some() {
                    self.send_reply(packet, edge_dhcp::MessageType::Offer, ip)
                        .await;
                }
            }
            Action::Request(ip, mac) => {
                let mac_lease = self.leaser.get_lease(*mac);
                let available = mac_lease
                    .map(|d| d.ip == ip || Instant::now() > d.expires)
                    .unwrap_or(true);

                let ip = (available
                    && self
                        .leaser
                        .add_lease(ip, *mac, Instant::now() + self.lease_time))
                .then_some(ip);

                let msg_type = match ip {
                    Some(_) => MessageType::Ack,
                    None => MessageType::Nak,
                };

                self.send_reply(packet, msg_type, ip).await;
            }
            Action::Release(_ip, mac) | Action::Decline(_ip, mac) => {
                self.leaser.remove_lease(*mac);
            }
        }
    }

    async fn send_reply(
        &mut self,
        packet: Packet<'_>,
        mt: MessageType,
        ip: Option<Ipv4Addr>,
        // TODO: add these to DhcpServer struct
        //gateways: &[Ipv4Addr],
        //subnet: Option<Ipv4Addr>,
        //dns: &[Ipv4Addr],
    ) {
        let mut opt_buf = Options::buf();
        let reply = packet.new_reply(
            ip,
            packet.options.reply(
                mt,
                self.ip,
                self.lease_time.as_secs() as u32,
                //gateways,
                //subnet,
                //dns,
                &[],
                None,
                &[],
                &mut opt_buf,
            ),
        );

        let mut buf = [0; DHCP_BUFFER_SIZE];
        let bytes_res = reply.encode(&mut buf);
        match bytes_res {
            Ok(bytes) => {
                let res = self.sock.send_to(bytes, DHCP_BROADCAST).await;
                if let Err(e) = res {
                    log::error!("Dhcp sock send error: {e:?}");
                }
            }
            Err(e) => {
                log::error!("Dhcp encode error: {e:?}");
            }
        }
    }

    fn get_packet_action<'b>(&self, packet: &'b Packet<'b>) -> Option<Action<'b>> {
        if packet.reply {
            return None;
        }

        let message_type = packet.options.iter().find_map(|option| match option {
            DhcpOption::MessageType(msg_type) => Some(msg_type),
            _ => None,
        });

        let message_type = message_type.or_else(|| {
            log::warn!("Ignoring DHCP request, no message type found: {packet:?}");
            None
        })?;

        let server_identifier = packet.options.iter().find_map(|option| match option {
            DhcpOption::ServerIdentifier(ip) => Some(ip),
            _ => None,
        });

        if server_identifier.is_some() && server_identifier != Some(self.ip) {
            log::warn!("Ignoring {message_type} request, not addressed to this server: {packet:?}");
            return None;
        }

        match message_type {
            MessageType::Discover => Some(Action::Discover(
                Self::get_requested_ip(&packet.options),
                &packet.chaddr,
            )),
            MessageType::Request => {
                let requested_ip =
                    Self::get_requested_ip(&packet.options).or_else(|| {
                        match packet.ciaddr.is_unspecified() {
                            true => None,
                            false => Some(packet.ciaddr),
                        }
                    })?;

                Some(Action::Request(requested_ip, &packet.chaddr))
            }
            MessageType::Release if server_identifier == Some(self.ip) => {
                Some(Action::Release(packet.yiaddr, &packet.chaddr))
            }
            MessageType::Decline if server_identifier == Some(self.ip) => {
                Some(Action::Decline(packet.yiaddr, &packet.chaddr))
            }
            _ => None,
        }
    }

    fn get_requested_ip<'b>(options: &'b Options<'b>) -> Option<Ipv4Addr> {
        options.iter().find_map(|option| {
            if let DhcpOption::RequestedIpAddress(ip) = option {
                Some(ip)
            } else {
                None
            }
        })
    }
}