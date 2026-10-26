//! The subscriber table (spec/areas/rate-feed.md#subscribers): addresses
//! proven by cookies, bounds in all and per client, expiry.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use core::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6};

use dassie_feed::{
    COOKIE_EPOCH_MS, Cookie, CookieCheck, CookieSecret, Decision, Ignored, Limits, Refused,
    SubscriberTable, encode_challenge, encode_subscribe,
};
use dassie_kit::ClientKey;

const NOW: u64 = 1_790_000_000_000;
const TTL: u64 = 300_000;

fn table(max_subscribers: usize, max_per_client: u32) -> SubscriberTable {
    SubscriberTable::new(
        Limits {
            max_subscribers,
            max_per_client,
            ttl_ms: TTL,
        },
        CookieSecret::from_bytes([3; 32]),
    )
}

fn v4(last: u8, port: u16) -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 0, 2, last)), port)
}

/// Subscribes `address` the way a client does: asks, takes the challenge,
/// asks again with its cookie.
fn join(table: &mut SubscriberTable, address: SocketAddr, now_ms: u64) -> Decision {
    let Decision::Challenge(cookie) = table.handle(address, &encode_subscribe(None), now_ms) else {
        panic!("a new address is challenged");
    };
    table.handle(address, &encode_subscribe(Some(&cookie)), now_ms)
}

#[test]
fn an_address_is_subscribed_once_it_sends_its_cookie_back() {
    let mut table = table(8, 2);
    let address = v4(1, 4000);
    let Decision::Challenge(cookie) = table.handle(address, &encode_subscribe(None), NOW) else {
        panic!("challenged");
    };
    assert!(!table.contains(address, NOW));
    assert_eq!(
        table.handle(address, &encode_subscribe(Some(&cookie)), NOW),
        Decision::Subscribed {
            new: true,
            refresh: None
        }
    );
    assert!(table.contains(address, NOW));
    assert_eq!(table.recipients(NOW).collect::<Vec<_>>(), [address]);
    let stats = table.stats();
    assert_eq!((stats.challenged, stats.subscribed), (1, 1));
}

#[test]
fn a_cookie_proves_only_its_own_address() {
    let mut table = table(8, 4);
    let victim = v4(1, 4000);
    let attacker = v4(2, 4000);
    let Decision::Challenge(cookie) = table.handle(attacker, &encode_subscribe(None), NOW) else {
        panic!("challenged");
    };
    // The attacker's cookie, sent from the victim's address (spoofed), or
    // from another port of the victim's, proves nothing.
    for from in [victim, v4(2, 4001)] {
        let decision = table.handle(from, &encode_subscribe(Some(&cookie)), NOW);
        assert!(matches!(decision, Decision::Challenge(_)), "{decision:?}");
    }
    let forged = Cookie::from_bytes([0; 16]);
    assert!(matches!(
        table.handle(victim, &encode_subscribe(Some(&forged)), NOW),
        Decision::Challenge(_)
    ));
    assert!(table.is_empty());
}

#[test]
fn an_ipv4_address_mapped_into_ipv6_has_the_same_cookie() {
    let secret = CookieSecret::from_bytes([3; 32]);
    let plain = v4(1, 4000);
    let mapped = SocketAddr::new(
        IpAddr::V6(Ipv4Addr::new(192, 0, 2, 1).to_ipv6_mapped()),
        4000,
    );
    let cookie = secret.issue(plain, NOW);
    assert_eq!(secret.check(mapped, &cookie, NOW), CookieCheck::Current);
}

#[test]
fn a_cookie_lives_one_to_two_epochs() {
    let secret = CookieSecret::from_bytes([3; 32]);
    let address = v4(1, 4000);
    let start = NOW - NOW % COOKIE_EPOCH_MS;
    let cookie = secret.issue(address, start + COOKIE_EPOCH_MS - 1);
    assert_eq!(secret.check(address, &cookie, start), CookieCheck::Current);
    assert_eq!(
        secret.check(address, &cookie, start + COOKIE_EPOCH_MS),
        CookieCheck::Previous
    );
    assert_eq!(
        secret.check(address, &cookie, start + 2 * COOKIE_EPOCH_MS - 1),
        CookieCheck::Previous
    );
    assert_eq!(
        secret.check(address, &cookie, start + 2 * COOKIE_EPOCH_MS),
        CookieCheck::Invalid
    );
    let other = CookieSecret::from_bytes([4; 32]);
    assert_eq!(other.check(address, &cookie, start), CookieCheck::Invalid);
    // At time zero there is no epoch before.
    let early = secret.issue(address, 0);
    assert_eq!(secret.check(address, &early, 0), CookieCheck::Current);
}

#[test]
fn a_renewal_extends_the_entry_and_refreshes_an_old_cookie() {
    let mut table = table(8, 2);
    let address = v4(1, 4000);
    let Decision::Challenge(cookie) = table.handle(address, &encode_subscribe(None), NOW) else {
        panic!("challenged");
    };
    table.handle(address, &encode_subscribe(Some(&cookie)), NOW);
    // A cookie from the epoch before is still taken, with a new one.
    let next_epoch = NOW - NOW % COOKIE_EPOCH_MS + COOKIE_EPOCH_MS;
    let later = next_epoch + COOKIE_EPOCH_MS - 1;
    let Decision::Subscribed {
        new: false,
        refresh: Some(fresh),
    } = table.handle(address, &encode_subscribe(Some(&cookie)), next_epoch)
    else {
        panic!("renewed with a refresh");
    };
    assert_eq!(
        table.handle(address, &encode_subscribe(Some(&fresh)), later),
        Decision::Subscribed {
            new: false,
            refresh: None
        }
    );
    assert!(table.contains(address, later + TTL - 1));
    assert!(!table.contains(address, later + TTL));
    assert_eq!(table.stats().renewed, 2);
}

#[test]
fn entries_expire_without_a_renewal() {
    let mut table = table(8, 2);
    join(&mut table, v4(1, 4000), NOW);
    join(&mut table, v4(2, 4000), NOW + 10);
    assert_eq!(table.next_expiry_ms(), Some(NOW + TTL));
    assert_eq!(
        table.recipients(NOW + TTL).collect::<Vec<_>>(),
        [v4(2, 4000)]
    );
    assert_eq!(table.expire(NOW + TTL - 1), 0);
    assert_eq!(table.expire(NOW + TTL), 1);
    assert_eq!(table.next_expiry_ms(), Some(NOW + 10 + TTL));
    assert_eq!(table.expire(u64::MAX), 1);
    assert!(table.is_empty());
    assert_eq!(table.next_expiry_ms(), None);
    assert_eq!(table.stats().expired, 2);
    assert_eq!(table.client_count(ClientKey::from_ip(v4(1, 0).ip())), 0);
}

#[test]
fn a_lapsed_entry_that_renews_before_it_is_dropped_comes_back_as_new() {
    let mut table = table(8, 2);
    let address = v4(1, 4000);
    join(&mut table, address, NOW);
    let lapsed = NOW + TTL;
    assert!(!table.contains(address, lapsed));
    assert_eq!(table.len(), 1, "not dropped yet");
    let Decision::Challenge(cookie) = table.handle(address, &encode_subscribe(None), lapsed) else {
        panic!("challenged");
    };
    // It got no feeds since it lapsed, so the caller sends it the latest.
    assert_eq!(
        table.handle(address, &encode_subscribe(Some(&cookie)), lapsed),
        Decision::Subscribed {
            new: true,
            refresh: None
        }
    );
    assert!(table.contains(address, lapsed));
    assert_eq!(table.stats().subscribed, 2);
    assert_eq!(table.stats().renewed, 0);
    table.check_invariants();
}

#[test]
fn a_client_has_a_share_of_the_table() {
    let mut table = table(8, 2);
    assert!(matches!(
        join(&mut table, v4(1, 1), NOW),
        Decision::Subscribed { new: true, .. }
    ));
    assert!(matches!(
        join(&mut table, v4(1, 2), NOW),
        Decision::Subscribed { new: true, .. }
    ));
    assert_eq!(
        join(&mut table, v4(1, 3), NOW),
        Decision::Refused(Refused::ClientFull)
    );
    assert!(matches!(
        join(&mut table, v4(2, 1), NOW),
        Decision::Subscribed { new: true, .. }
    ));

    // Every address of an IPv6 /56 is one client.
    let in_56 = |last: u16| {
        SocketAddr::new(
            IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0xff, last, 0, 0, 1)),
            4000,
        )
    };
    assert!(matches!(
        join(&mut table, in_56(1), NOW),
        Decision::Subscribed { .. }
    ));
    assert!(matches!(
        join(&mut table, in_56(2), NOW),
        Decision::Subscribed { .. }
    ));
    assert_eq!(
        join(&mut table, in_56(3), NOW),
        Decision::Refused(Refused::ClientFull)
    );
    assert_eq!(table.stats().refused_client, 2);
}

#[test]
fn a_full_table_refuses_newcomers_until_an_entry_expires() {
    let mut table = table(3, 4);
    for last in 1..=3 {
        join(&mut table, v4(last, 4000), NOW);
    }
    assert_eq!(
        join(&mut table, v4(9, 4000), NOW + 1),
        Decision::Refused(Refused::Full)
    );
    // Renewals still go through.
    let cookie = CookieSecret::from_bytes([3; 32]).issue(v4(1, 4000), NOW + 1);
    assert!(matches!(
        table.subscribe(v4(1, 4000), Some(&cookie), NOW + 1),
        Decision::Subscribed { new: false, .. }
    ));
    // Once the others expired, the newcomer gets in without a call to
    // expire first.
    assert!(matches!(
        join(&mut table, v4(9, 4000), NOW + TTL),
        Decision::Subscribed { new: true, .. }
    ));
    assert_eq!(table.len(), 2);
    assert_eq!(table.stats().refused_full, 1);
}

#[test]
fn a_flow_label_does_not_make_another_entry() {
    let mut table = table(8, 4);
    let ip = Ipv6Addr::new(0x2001, 0xdb8, 0, 1, 0, 0, 0, 1);
    let plain = SocketAddr::V6(SocketAddrV6::new(ip, 4000, 0, 0));
    let labelled = SocketAddr::V6(SocketAddrV6::new(ip, 4000, 77, 0));
    join(&mut table, plain, NOW);
    assert!(matches!(
        join(&mut table, labelled, NOW),
        Decision::Subscribed { new: false, .. }
    ));
    assert_eq!(table.len(), 1);
    assert!(table.contains(labelled, NOW));
    assert_eq!(table.recipients(NOW).collect::<Vec<_>>(), [plain]);
}

#[test]
fn a_link_local_subscriber_keeps_its_scope() {
    let mut table = table(8, 4);
    let ip = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1);
    let scoped = SocketAddr::V6(SocketAddrV6::new(ip, 4000, 77, 3));
    join(&mut table, scoped, NOW);
    assert_eq!(
        table.recipients(NOW).collect::<Vec<_>>(),
        [SocketAddr::V6(SocketAddrV6::new(ip, 4000, 0, 3))]
    );
}

#[test]
fn an_address_a_feed_cannot_go_to_gets_no_answer() {
    let mut table = table(8, 4);
    let sources = [
        v4(1, 0),
        SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 4000),
        SocketAddr::new(Ipv4Addr::BROADCAST.into(), 4000),
        SocketAddr::new(Ipv4Addr::new(224, 0, 0, 1).into(), 4000),
        SocketAddr::new(Ipv6Addr::UNSPECIFIED.into(), 4000),
        SocketAddr::new(Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 1).into(), 4000),
        SocketAddr::new(Ipv4Addr::BROADCAST.to_ipv6_mapped().into(), 4000),
    ];
    for from in sources {
        assert_eq!(
            table.handle(from, &encode_subscribe(None), NOW),
            Decision::Ignored(Ignored::NotUnicast),
            "{from}"
        );
    }
    assert_eq!(table.stats().challenged, 0);
}

#[test]
fn only_a_subscribe_gets_an_answer() {
    let mut table = table(8, 4);
    let address = v4(1, 4000);
    let challenge = encode_challenge(&Cookie::from_bytes([1; 16]));
    assert_eq!(
        table.handle(address, &challenge, NOW),
        Decision::Ignored(Ignored::NotSubscribe)
    );
    assert!(matches!(
        table.handle(address, &[1, 2, 3], NOW),
        Decision::Ignored(Ignored::Malformed(_))
    ));
    assert_eq!(table.stats().ignored, 2);
    assert!(table.is_empty());
}
