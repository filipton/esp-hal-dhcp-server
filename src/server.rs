use edge_dhcp::{server::Action, DhcpOption, Ipv4Addr, MessageType, Options, Packet};
use embassy_net::udp::UdpSocket;
use embassy_time::Instant;

use crate::structs::{
    DhcpLeaser, DhcpServerConfig, DHCP_BROADCAST, DHCP_BUFFER_SIZE, DHCP_SERVER_ENDPOINT,
};

pub struct DhcpServer<'a> {
    pub config: DhcpServerConfig<'a>,

    leaser: &'a mut dyn DhcpLeaser,
    sock: UdpSocket<'a>,
}

impl<'a> DhcpServer<'a> {
    pub fn new(
        config: DhcpServerConfig<'a>,
        leaser: &'a mut dyn DhcpLeaser,
        mut sock: UdpSocket<'a>,
    ) -> Self {
        sock.bind(DHCP_SERVER_ENDPOINT).unwrap();

        Self {
            config,
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
                        .add_lease(ip, *mac, Instant::now() + self.config.lease_time))
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

    async fn send_reply(&mut self, packet: Packet<'_>, mt: MessageType, ip: Option<Ipv4Addr>) {
        let mut opt_buf = Options::buf();
        let reply = packet.new_reply(
            ip,
            packet.options.reply(
                mt,
                self.config.ip,
                self.config.lease_time.as_secs() as u32,
                self.config.gateways,
                self.config.subnet,
                self.config.dns,
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

        if server_identifier.is_some() && server_identifier != Some(self.config.ip) {
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
            MessageType::Release if server_identifier == Some(self.config.ip) => {
                Some(Action::Release(packet.yiaddr, &packet.chaddr))
            }
            MessageType::Decline if server_identifier == Some(self.config.ip) => {
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
