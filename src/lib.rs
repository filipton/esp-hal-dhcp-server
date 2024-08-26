#![no_std]

pub use edge_dhcp::Ipv4Addr;
use embassy_net::{
    driver::Driver,
    udp::{PacketMetadata, UdpSocket},
    Stack,
};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::Instant;
pub use server::DhcpServer;
pub use structs::DhcpServerConfig;
use structs::{DhcpLease, DhcpLeaser};

mod server;
mod structs;

pub type CloseSignal = Signal<CriticalSectionRawMutex, ()>;
pub static CLOSE_SIGNAL: CloseSignal = Signal::new();

pub async fn run_dhcp_server<D: Driver>(
    stack: &'static Stack<D>,
    config: DhcpServerConfig<'_>,
    leaser: &'_ mut dyn DhcpLeaser,
) {
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

    let mut server = DhcpServer::new(config, leaser, sock);
    embassy_futures::join::join(server.run(), CLOSE_SIGNAL.wait()).await;
}

pub struct SimpleDhcpLeaser {
    pub start: Ipv4Addr,
    pub end: Ipv4Addr,
    pub leases: heapless::Vec<DhcpLease, 16>,
}

impl DhcpLeaser for SimpleDhcpLeaser {
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
