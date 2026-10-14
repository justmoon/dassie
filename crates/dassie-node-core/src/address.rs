//! Which addresses the node may send peer requests to (spec 7 §4.7).
//!
//! A peer's URL comes from its signed link state, which anyone registered
//! can make say anything. Without a filter, the node could be made to send
//! requests into its own network: to the loopback interface, a router, or
//! a cloud provider's metadata service. So requests go only to public
//! addresses, checked after DNS resolution, unless the operator allows
//! private ones (dev and tests).

use core::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Whether `address` is a public unicast address.
#[must_use]
pub fn is_public(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => is_public_v6(v6),
    }
}

fn is_public_v4(address: Ipv4Addr) -> bool {
    let [a, b, c, _] = address.octets();
    let private = a == 0                               // "this network"
        || a == 10                                     // private
        || (a == 100 && (64..128).contains(&b))        // CGNAT
        || a == 127                                    // loopback
        || (a == 169 && b == 254)                      // link-local, cloud metadata
        || (a == 172 && (16..32).contains(&b))         // private
        || (a == 192 && b == 0 && c == 0)              // IETF protocol assignments
        || (a == 192 && b == 0 && c == 2)              // documentation
        || (a == 192 && b == 168)                      // private
        || (a == 198 && (b == 18 || b == 19))          // benchmarking
        || (a == 198 && b == 51 && c == 100)           // documentation
        || (a == 203 && b == 0 && c == 113)            // documentation
        || a >= 224; // multicast, reserved, broadcast
    !private
}

fn is_public_v6(address: Ipv6Addr) -> bool {
    if let Some(v4) = embedded_v4(address) {
        return is_public_v4(v4);
    }
    let segments = address.segments();
    let [first, second, ..] = segments;
    let private = address.is_unspecified()
        || address.is_loopback()
        || (first & 0xfe00) == 0xfc00                  // unique local
        || (first & 0xffc0) == 0xfe80                  // link-local
        || (first & 0xffc0) == 0xfec0                  // site-local
        || (first & 0xff00) == 0xff00                  // multicast
        || matches!(segments, [0x0100, 0, 0, 0, ..])      // discard
        || (first == 0x2001 && second == 0x0db8); // documentation
    !private
}

/// The IPv4 address an IPv6 address carries, in the forms that route to
/// it: mapped, NAT64, 6to4 and Teredo.
fn embedded_v4(address: Ipv6Addr) -> Option<Ipv4Addr> {
    if let Some(v4) = address.to_ipv4_mapped() {
        return Some(v4);
    }
    let s = address.segments();
    let from = |high: u16, low: u16| {
        let [a, b] = high.to_be_bytes();
        let [c, d] = low.to_be_bytes();
        Ipv4Addr::new(a, b, c, d)
    };
    match s {
        // NAT64, well-known and local-use prefixes.
        [0x0064, 0xff9b, 0, 0, 0, 0, high, low] | [0x0064, 0xff9b, 0x0001, _, _, _, high, low] => {
            Some(from(high, low))
        }
        // 6to4.
        [0x2002, high, low, ..] => Some(from(high, low)),
        // Teredo: the client's address, inverted.
        [0x2001, 0, _, _, _, _, high, low] => Some(from(!high, !low)),
        _ => None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn public(text: &str) -> bool {
        is_public(text.parse().unwrap())
    }

    #[test]
    fn private_and_special_addresses_are_refused() {
        for refused in [
            "0.0.0.0",
            "10.1.2.3",
            "100.64.0.1",
            "127.0.0.1",
            "169.254.169.254",
            "172.16.0.1",
            "172.31.255.255",
            "192.0.0.8",
            "192.0.2.1",
            "192.168.1.1",
            "198.18.0.1",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "255.255.255.255",
            "::",
            "::1",
            "fc00::1",
            "fd12::1",
            "fe80::1",
            "fec0::1",
            "ff02::1",
            "100::1",
            "2001:db8::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "64:ff9b::a9fe:a9fe",
            "2002:7f00:1::",
            "2001:0:4136:e378:8000:63bf:80ff:fffe",
        ] {
            assert!(!public(refused), "{refused}");
        }
    }

    #[test]
    fn public_addresses_pass() {
        for allowed in [
            "1.1.1.1",
            "8.8.8.8",
            "100.128.0.1",
            "172.32.0.1",
            "2606:4700::1111",
            "::ffff:1.1.1.1",
            "2002:0101:0101::",
        ] {
            assert!(public(allowed), "{allowed}");
        }
    }
}
