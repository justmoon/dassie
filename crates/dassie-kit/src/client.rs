//! Who a request comes from, for bounds per client: an IPv4 address, or
//! an IPv6 /56, since one customer of a provider usually has a whole /56.

use core::net::IpAddr;

/// Who a request comes from: an IPv4 address, or the /56 of an IPv6
/// address, since one customer of a provider usually has a whole /56.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ClientKey([u8; 16]);

impl ClientKey {
    /// The key of `address`.
    #[must_use]
    pub fn from_ip(address: IpAddr) -> Self {
        match address {
            IpAddr::V4(v4) => Self(v4.to_ipv6_mapped().octets()),
            IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
                Some(v4) => Self(v4.to_ipv6_mapped().octets()),
                None => {
                    let mut octets = v6.octets();
                    for byte in octets.iter_mut().skip(7) {
                        *byte = 0;
                    }
                    Self(octets)
                }
            },
        }
    }

    /// The key of `address`'s network: its IPv4 /24, or its IPv6 /56. An
    /// IPv4 address mapped into IPv6, as a dual-stack socket reports IPv4
    /// clients, is the IPv4 address. For shares of a pool rather than
    /// rates: a /24 is the least IPv4 block routed on its own, so one
    /// party rarely has addresses in many.
    #[must_use]
    pub fn network_of(address: IpAddr) -> Self {
        match address.to_canonical() {
            IpAddr::V4(v4) => {
                let [a, b, c, _] = v4.octets();
                Self(
                    core::net::Ipv4Addr::new(a, b, c, 0)
                        .to_ipv6_mapped()
                        .octets(),
                )
            }
            v6 @ IpAddr::V6(_) => Self::from_ip(v6),
        }
    }

    /// A key of `bytes` that names something other than an address, such
    /// as a node by its key: a table keyed so never holds addresses.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// The key as bytes, as a database keeps it.
    #[must_use]
    pub const fn to_bytes(self) -> [u8; 16] {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use core::net::{Ipv4Addr, Ipv6Addr};

    use super::*;

    fn v4(last: u8) -> ClientKey {
        ClientKey::from_ip(IpAddr::V4(Ipv4Addr::new(10, 0, 0, last)))
    }

    #[test]
    fn ipv6_clients_are_grouped_by_slash_56() {
        let a = Ipv6Addr::new(0x2001, 0xdb8, 0x12, 0x3401, 0, 0, 0, 1);
        let b = Ipv6Addr::new(0x2001, 0xdb8, 0x12, 0x34ff, 0xffff, 0, 0, 2);
        let other = Ipv6Addr::new(0x2001, 0xdb8, 0x12, 0x3501, 0, 0, 0, 1);
        assert_eq!(
            ClientKey::from_ip(IpAddr::V6(a)),
            ClientKey::from_ip(IpAddr::V6(b))
        );
        assert_ne!(
            ClientKey::from_ip(IpAddr::V6(a)),
            ClientKey::from_ip(IpAddr::V6(other))
        );
        let mapped = Ipv4Addr::new(10, 0, 0, 1).to_ipv6_mapped();
        assert_eq!(ClientKey::from_ip(IpAddr::V6(mapped)), v4(1));
    }

    #[test]
    fn networks_are_an_ipv4_slash_24_or_an_ipv6_slash_56() {
        let network = |address: IpAddr| ClientKey::network_of(address);
        let a = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1));
        let b = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 254));
        let other = IpAddr::V4(Ipv4Addr::new(192, 0, 3, 1));
        assert_eq!(network(a), network(b));
        assert_ne!(network(a), network(other));
        let mapped = IpAddr::V6(Ipv4Addr::new(192, 0, 2, 9).to_ipv6_mapped());
        assert_eq!(network(mapped), network(a));
        let v6 = Ipv6Addr::new(0x2001, 0xdb8, 0x12, 0x3401, 0, 0, 0, 1);
        let same = Ipv6Addr::new(0x2001, 0xdb8, 0x12, 0x34ff, 0xffff, 0, 0, 2);
        assert_eq!(network(IpAddr::V6(v6)), network(IpAddr::V6(same)));
        assert_eq!(network(IpAddr::V6(v6)), ClientKey::from_ip(IpAddr::V6(v6)));
    }
}
