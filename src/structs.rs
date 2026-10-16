use edge_dhcp::Ipv4Addr;
use embassy_net::{IpAddress, IpEndpoint};
use embassy_time::{Duration, Instant};

pub const DHCP_BROADCAST: IpEndpoint = IpEndpoint::new(IpAddress::v4(255, 255, 255, 255), 68);
pub const DHCP_SERVER_ENDPOINT: IpEndpoint = IpEndpoint::new(IpAddress::v4(0, 0, 0, 0), 67);
pub const DHCP_BUFFER_SIZE: usize = 1024;

#[derive(Debug, Clone)]
pub struct DhcpLease {
    pub ip: Ipv4Addr,
    pub mac: [u8; 16],
    pub expires: Instant,
}

/// Address allocation strategy used by the server.
pub trait DhcpLeaser {
    /// The lease currently recorded for `mac` (it may be expired).
    fn get_lease(&mut self, mac: [u8; 16]) -> Option<DhcpLease>;
    /// A free address to offer to a new client, if any.
    fn next_lease(&mut self) -> Option<Ipv4Addr>;
    /// Records a lease; `false` makes the server answer the request with NAK.
    fn add_lease(&mut self, ip: Ipv4Addr, mac: [u8; 16], expires: Instant) -> bool;
    fn remove_lease(&mut self, mac: [u8; 16]) -> bool;
}

#[derive(Debug, Clone)]
pub struct DhcpServerConfig<'a> {
    pub ip: Ipv4Addr,
    pub lease_time: Duration,

    pub gateways: &'a [Ipv4Addr],
    pub subnet: Option<Ipv4Addr>,
    pub dns: &'a [Ipv4Addr],

    pub use_captive_portal: bool,
}
