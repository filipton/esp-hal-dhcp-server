use crate::structs::{DhcpLease, DhcpLeaser};
use edge_dhcp::Ipv4Addr;
use embassy_time::Instant;

/// Leases addresses from the `start..=end` range, holding up to 16 leases.
///
/// Expired leases are reused, and an address outside the range or still leased to another
/// client is refused (the server then answers with NAK).
pub struct SimpleDhcpLeaser {
    pub start: Ipv4Addr,
    pub end: Ipv4Addr,
    pub leases: heapless::Vec<DhcpLease, 16>,
}

impl SimpleDhcpLeaser {
    fn in_range(&self, ip: Ipv4Addr) -> bool {
        (u32::from(self.start)..=u32::from(self.end)).contains(&u32::from(ip))
    }

    fn is_taken(&self, ip: Ipv4Addr, now: Instant) -> bool {
        self.leases.iter().any(|l| l.ip == ip && l.expires > now)
    }
}

impl DhcpLeaser for SimpleDhcpLeaser {
    fn get_lease(&mut self, mac: [u8; 16]) -> Option<DhcpLease> {
        self.leases.iter().find(|l| l.mac == mac).cloned()
    }

    fn next_lease(&mut self) -> Option<Ipv4Addr> {
        let now = Instant::now();
        if self.leases.is_full() && self.leases.iter().all(|l| l.expires > now) {
            return None;
        }

        (u32::from(self.start)..=u32::from(self.end))
            .map(Ipv4Addr::from)
            .find(|ip| !self.is_taken(*ip, now))
    }

    fn add_lease(&mut self, ip: Ipv4Addr, mac: [u8; 16], expires: Instant) -> bool {
        let now = Instant::now();
        if !self.in_range(ip)
            || self
                .leases
                .iter()
                .any(|l| l.ip == ip && l.mac != mac && l.expires > now)
        {
            return false;
        }

        // drop this client's old lease, anything else holding `ip` (expired) and expired leases
        self.leases
            .retain(|l| l.mac != mac && l.ip != ip && l.expires > now);
        self.leases.push(DhcpLease { ip, mac, expires }).is_ok()
    }

    fn remove_lease(&mut self, mac: [u8; 16]) -> bool {
        let len = self.leases.len();
        self.leases.retain(|l| l.mac != mac);
        self.leases.len() != len
    }
}

/// Always offers the same single address (for one client at a time).
pub struct SingleDhcpLeaser {
    pub ip: Ipv4Addr,
}

impl SingleDhcpLeaser {
    pub fn new(ip: Ipv4Addr) -> Self {
        Self { ip }
    }
}

impl DhcpLeaser for SingleDhcpLeaser {
    fn get_lease(&mut self, _mac: [u8; 16]) -> Option<DhcpLease> {
        None
    }

    fn next_lease(&mut self) -> Option<Ipv4Addr> {
        Some(self.ip)
    }

    fn add_lease(&mut self, ip: Ipv4Addr, _mac: [u8; 16], _expires: Instant) -> bool {
        ip == self.ip
    }

    fn remove_lease(&mut self, _mac: [u8; 16]) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embassy_time::Duration;

    fn mac(n: u8) -> [u8; 16] {
        let mut m = [0; 16];
        m[0] = n;
        m
    }

    fn leaser() -> SimpleDhcpLeaser {
        SimpleDhcpLeaser {
            start: Ipv4Addr::new(10, 0, 0, 10),
            end: Ipv4Addr::new(10, 0, 0, 40),
            leases: heapless::Vec::new(),
        }
    }

    fn later() -> Instant {
        Instant::now() + Duration::from_secs(60)
    }

    #[test]
    fn hands_out_distinct_addresses() {
        let mut l = leaser();
        let a = l.next_lease().unwrap();
        assert!(l.add_lease(a, mac(1), later()));
        let b = l.next_lease().unwrap();
        assert_ne!(a, b);
        assert!(l.add_lease(b, mac(2), later()));
        assert_eq!(l.get_lease(mac(1)).unwrap().ip, a);
    }

    #[test]
    fn refuses_foreign_and_taken_addresses() {
        let mut l = leaser();
        // e.g. an INIT-REBOOT request for the address from the client's home network
        assert!(!l.add_lease(Ipv4Addr::new(192, 168, 1, 50), mac(1), later()));
        let a = Ipv4Addr::new(10, 0, 0, 10);
        assert!(l.add_lease(a, mac(1), later()));
        assert!(!l.add_lease(a, mac(2), later()));
        // renewing the own lease is fine
        assert!(l.add_lease(a, mac(1), later()));
        assert_eq!(l.leases.len(), 1);
    }

    #[test]
    fn reuses_expired_leases_when_full() {
        let mut l = leaser();
        let soon = Instant::now() + Duration::from_millis(20);
        for i in 0..16 {
            let ip = l.next_lease().unwrap();
            assert!(l.add_lease(ip, mac(i), soon));
        }
        assert!(l.leases.is_full());
        std::thread::sleep(core::time::Duration::from_millis(40));

        // all expired: a new client still gets an address
        let ip = l.next_lease().unwrap();
        assert!(l.add_lease(ip, mac(100), later()));
        assert_eq!(l.get_lease(mac(100)).unwrap().ip, ip);
    }

    #[test]
    fn full_of_active_leases_offers_nothing() {
        let mut l = leaser();
        for i in 0..16 {
            let ip = l.next_lease().unwrap();
            assert!(l.add_lease(ip, mac(i), later()));
        }
        assert_eq!(l.next_lease(), None);
    }

    #[test]
    fn remove_lease_frees_address() {
        let mut l = leaser();
        let a = l.next_lease().unwrap();
        assert!(l.add_lease(a, mac(1), later()));
        assert!(l.remove_lease(mac(1)));
        assert!(!l.remove_lease(mac(1)));
        assert_eq!(l.next_lease(), Some(a));
    }
}
