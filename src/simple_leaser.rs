use crate::structs::{DhcpLease, DhcpLeaser};
use edge_dhcp::Ipv4Addr;
use embassy_time::Instant;

// TODO: maybe use embassy ipv4 (with custom ->u32 iplementation)
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
