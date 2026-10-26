//! Arbitrary frames never make an RPC connection panic, and anything that is
//! not a `ClientMessage` ends it as a protocol error; nor do the owner's
//! calls with arbitrary fields make a handler panic
//! (spec/decisions/tigerstyle.md, spec/areas/testing.md#property-tests).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::disallowed_types,
    clippy::indexing_slicing
)]

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use dassie_identity::NodeKey;
use dassie_kit::sim::SeededRng;
use dassie_node::clock::ManualClock;
use dassie_node::node::{Node, NodeOptions, PeerOptions, Policies, Runtime};
use dassie_node::open_payments::Urls;
use dassie_node::rpc::connection::{self, Closed};
use dassie_node::rpc::proto::{request::Method, server_message};
use dassie_node::rpc::{Caller, is_live, proto};
use dassie_node::store::Store;
use dassie_node::tls;
use dassie_node_core::config::{DirectoryUrl, Hostname};
use dassie_payments::pointer::Recipient;
use futures_util::StreamExt;
use proptest::prelude::*;
use prost::Message;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

fn node() -> Arc<Node> {
    node_on(Store::open_in_memory().unwrap())
}

fn node_on(store: Store) -> Arc<Node> {
    Node::new(NodeOptions {
        store,
        clock: Arc::new(ManualClock::new(1_800_000_000_000)),
        rng: Box::new(SeededRng::new(1)),
        runtime: Runtime::Systemd,
        http_port: 80,
        https_port: 443,
        acme_ca_file: None,
        policies: Policies::DEFAULT,
        peer: PeerOptions::default(),
        dev_wallet_dir: None,
        public_names: Vec::new(),
        live_realm: dassie_node_core::config::LiveRealm::Refused,
    })
    .unwrap()
}

fn serve(node: &Arc<Node>, runtime: &tokio::runtime::Runtime, frames: Vec<Vec<u8>>) -> Closed {
    let incoming = futures_util::stream::iter(
        frames
            .into_iter()
            .map(|frame| Ok::<_, Infallible>(Bytes::from(frame))),
    );
    let outgoing = futures_util::sink::drain();
    let caller = Caller::Browser {
        session: None,
        loopback: false,
    };
    runtime.block_on(connection::serve(
        Arc::clone(node),
        caller,
        incoming,
        outgoing,
        CancellationToken::new(),
    ))
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    #[test]
    fn arbitrary_frames(frames in proptest::collection::vec(proptest::collection::vec(any::<u8>(), 0..64), 0..8)) {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let node = node();
        let malformed = frames.iter().any(|frame| match proto::ClientMessage::decode(frame.as_slice()) {
            Ok(message) => message.kind.is_none(),
            Err(_) => true,
        });
        let closed = serve(&node, &runtime, frames);
        if malformed {
            prop_assert_eq!(closed, Closed::ProtocolError);
        } else {
            prop_assert!(matches!(closed, Closed::ByClient | Closed::ProtocolError));
        }
    }
}

// A flood of calls the node refuses (no method) once filled the outgoing
// queue, which only the connection's own loop drains, while that loop
// waited for room in it: the connection hung for ever, deaf to shutdown.
#[test]
fn a_flood_of_refused_calls_ends_the_connection() {
    // Paused time: the send timeout passes as soon as nothing else can run.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .start_paused(true)
        .build()
        .unwrap();
    let node = node();
    let frames = (0..200_000_u32).map(|call_id| {
        let message = proto::ClientMessage {
            call_id,
            kind: Some(proto::client_message::Kind::Request(proto::Request {
                method: None,
            })),
        };
        Ok::<_, Infallible>(Bytes::from(message.encode_to_vec()))
    });
    let incoming = futures_util::stream::iter(frames);
    // A client that never reads: every send waits.
    let outgoing = futures_util::sink::unfold((), |(), _: Bytes| {
        futures_util::future::pending::<Result<(), Infallible>>()
    });
    let serve = connection::serve(
        Arc::clone(&node),
        Caller::Browser {
            session: None,
            loopback: false,
        },
        incoming,
        Box::pin(outgoing),
        CancellationToken::new(),
    );
    let closed = runtime
        .block_on(async { tokio::time::timeout(std::time::Duration::from_secs(60), serve).await });
    assert!(
        matches!(closed, Ok(Closed::ProtocolError | Closed::ByClient)),
        "{closed:?}"
    );
}

// The owner's calls get past authentication, so their fields reach the
// handlers' parsing. The node is set up, with a hostname and the stub
// ledger, so that the methods behind those answer instead of refusing.
// A handler's panic is caught and answered as INTERNAL, which only bugs
// are: the property looks at the answers, not at how the connection ended.

/// The node's hostname.
const HOSTNAME: &str = "node.example";

/// Its key.
const KEY: [u8; 32] = [1; 32];

/// How long the calls of one case may take to answer.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(20);

/// Hosts the outbound client refuses before any IO: IP literals that are not
/// public.
const PRIVATE_HOSTS: [&str; 4] = ["127.0.0.1", "[::1]", "10.0.0.1:8443", "[fd00::1]:443"];

/// A set-up node, with its engine running on `runtime` until `shutdown`.
fn running_node(
    runtime: &tokio::runtime::Runtime,
    shutdown: &CancellationToken,
) -> (Arc<Node>, tokio::task::JoinHandle<()>) {
    let store = Store::open_in_memory().unwrap();
    store
        .set_node_key_once(&NodeKey::from_bytes(&KEY), 1_800_000_000_000)
        .unwrap();
    let node = node_on(store);
    node.set_hostname(HOSTNAME).unwrap();
    let engine = runtime.spawn(dassie_node::engine::run(
        Arc::clone(&node),
        shutdown.clone(),
    ));
    runtime.block_on(async {
        let engine = loop {
            if let Some(engine) = node.engine() {
                break engine;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        };
        engine
            .add_scheme("stub".into(), String::new())
            .await
            .unwrap();
    });
    (node, engine)
}

/// Serves `calls` as the owner, a frame each, and ends the connection once
/// every call has answered: a unary call with its response or error, a
/// live one with anything. Returns why it ended and what the node sent.
fn serve_owner(
    node: &Arc<Node>,
    runtime: &tokio::runtime::Runtime,
    calls: &[(u32, Method)],
) -> (Closed, Vec<proto::ServerMessage>) {
    let frames: Vec<_> = calls
        .iter()
        .map(|(call_id, method)| {
            let message = proto::ClientMessage {
                call_id: *call_id,
                kind: Some(proto::client_message::Kind::Request(proto::Request {
                    method: Some(method.clone()),
                })),
            };
            Ok::<_, Infallible>(Bytes::from(message.encode_to_vec()))
        })
        .collect();
    let expected = expected(calls);
    let (sent, mut seen) = watch::channel(Vec::new());
    let messages = seen.clone();
    let outgoing = futures_util::sink::unfold(sent, |sent, bytes: Bytes| async move {
        let message = proto::ServerMessage::decode(bytes).unwrap();
        sent.send_modify(|messages| messages.push(message));
        Ok::<_, Infallible>(sent)
    });
    let done = async move {
        let all = seen.wait_for(|messages| answered(&expected, messages));
        let _ = tokio::time::timeout(ANSWER_TIMEOUT, all).await;
    };
    let tail = futures_util::stream::once(done).filter_map(|()| async { None });
    let incoming = futures_util::stream::iter(frames).chain(tail);
    let closed = runtime.block_on(connection::serve(
        Arc::clone(node),
        Caller::Owner,
        Box::pin(incoming),
        Box::pin(outgoing),
        CancellationToken::new(),
    ));
    let messages = messages.borrow().clone();
    (closed, messages)
}

/// Each call, and whether it is live.
fn expected(calls: &[(u32, Method)]) -> Vec<(u32, bool)> {
    calls
        .iter()
        .map(|(call_id, method)| (*call_id, is_live(method)))
        .collect()
}

/// Whether every call answered.
fn answered(calls: &[(u32, bool)], messages: &[proto::ServerMessage]) -> bool {
    calls.iter().all(|&(call_id, live)| {
        messages.iter().any(|message| {
            message.call_id == call_id
                && (live || !matches!(message.kind, Some(server_message::Kind::Item(_))))
        })
    })
}

/// Text that trips parsers: odd characters, control and direction
/// characters, nothing, and more than any field allows.
fn text() -> BoxedStrategy<String> {
    prop_oneof![
        4 => any::<String>(),
        3 => "[ -~]{0,48}",
        1 => Just(String::new()),
        1 => "[\\x00-\\x1f\\x7f\\u{200e}\\u{202e}\\u{2066}\\u{feff}\\u{fffd}\\u{10ffff}]{1,8}",
        1 => (any::<char>(), 257_usize..70_000)
            .prop_map(|(c, n)| std::iter::repeat_n(c, n).collect()),
    ]
    .boxed()
}

/// One of `known`, changed a little: in case, with space around it, or
/// with a character replaced, inserted or cut off.
fn near(known: &'static [&'static str]) -> BoxedStrategy<String> {
    (
        proptest::sample::select(known),
        0_u8..6,
        any::<proptest::sample::Index>(),
        any::<char>(),
    )
        .prop_map(|(known, how, at, c)| {
            let mut chars: Vec<char> = known.chars().collect();
            let at = at.index(chars.len() + 1);
            match how {
                0 => return known.to_ascii_uppercase(),
                1 => return format!(" {known}\t"),
                2 if at < chars.len() => chars[at] = c,
                3 => chars.insert(at, c),
                4 => chars.truncate(at),
                _ => return known.to_string(),
            }
            chars.into_iter().collect()
        })
        .boxed()
}

/// A field that should be one of `known`: one of them, nearly, or any text.
fn field(known: &'static [&'static str]) -> BoxedStrategy<String> {
    prop_oneof![2 => near(known), 1 => text()].boxed()
}

/// Numbers at the edges, small ones, and any.
fn number() -> BoxedStrategy<u64> {
    prop_oneof![
        proptest::sample::select(vec![0, 1, u64::from(u32::MAX), 1 << 63, u64::MAX]),
        0_u64..1_000,
        any::<u64>(),
    ]
    .boxed()
}

fn small() -> BoxedStrategy<u32> {
    prop_oneof![
        proptest::sample::select(vec![0, 1, 65_535, 65_536, u32::MAX]),
        any::<u32>(),
    ]
    .boxed()
}

const LEDGERS: &[&str] = &["stub+usd", "xrpl-testnet+xrp", "xrpl+xrp", "stub"];

/// The IDs of payments, quotes and grants, UUIDs.
fn payment_id() -> BoxedStrategy<String> {
    prop_oneof![
        "[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}",
        "[0-9a-f+-]{8}-[0-9a-f+ ]{4}-[0-9a-f-]{4}-[0-9a-f]{4}-[0-9a-f+]{12}",
        "\\PC{8}-\\PC{4}-\\PC{4}-\\PC{4}-\\PC{12}",
        "[0-9a-f-]{30,40}",
        text(),
    ]
    .boxed()
}

fn node_id() -> BoxedStrategy<String> {
    let own = NodeKey::from_bytes(&KEY).node_id().as_str().to_string();
    prop_oneof![
        Just(own),
        "[A-Za-z0-9_-]{0,48}",
        "[A-Za-z0-9_=+/.-]{18,24}",
        text(),
    ]
    .boxed()
}

/// A hostname: any text, but no valid one other than the node's own, so
/// that its wallet addresses stay its own (see [`recipient`]).
fn hostname() -> BoxedStrategy<String> {
    field(&[
        HOSTNAME,
        "localhost",
        "a.b",
        "-a.example",
        "xn--bcher-kva.example",
        "127.0.0.1",
    ])
    .prop_map(|name| match Hostname::parse(&name) {
        Ok(parsed) if parsed.as_str() != HOSTNAME => HOSTNAME.to_ascii_uppercase(),
        _ => name,
    })
    .boxed()
}

/// An ACME directory that is refused: a valid one, or none, which means
/// the default, starts an order over the network.
fn refused_directory() -> BoxedStrategy<String> {
    field(&[
        "https://acme.example/directory",
        "https://127.0.0.1:14000/dir",
    ])
    .prop_map(|url| {
        if url.is_empty() || DirectoryUrl::parse(&url).is_ok() {
            "ftp://acme.example/".into()
        } else {
            url
        }
    })
    .boxed()
}

/// A settlement scheme, but not the XRP Ledger's: it connects to the
/// ledger over the network.
fn scheme_id() -> BoxedStrategy<String> {
    field(&["stub", dassie_xrpl::scheme::SCHEME_ID, ""])
        .prop_map(|id| {
            if id.to_ascii_lowercase().contains("xrpl") {
                "stub".into()
            } else {
                id
            }
        })
        .boxed()
}

/// PEM: any text, blocks of any base64, and a real certificate and key,
/// whole or with a character changed.
fn pem() -> BoxedStrategy<String> {
    use base64::Engine as _;
    let labels = [
        "CERTIFICATE",
        "PRIVATE KEY",
        "EC PRIVATE KEY",
        "RSA PRIVATE KEY",
        "X",
    ];
    let block = (
        proptest::sample::select(labels.to_vec()),
        proptest::collection::vec(any::<u8>(), 0..300),
    )
        .prop_map(|(label, body)| {
            let body = base64::engine::general_purpose::STANDARD.encode(body);
            format!("-----BEGIN {label}-----\n{body}\n-----END {label}-----\n")
        });
    let own = tls::self_signed(&[HOSTNAME.to_string()], 1_800_000_000_000).unwrap();
    let real = proptest::sample::select(vec![own.chain_pem, own.key_pem]);
    let changed = (
        real.clone(),
        any::<proptest::sample::Index>(),
        any::<char>(),
    )
        .prop_map(|(pem, at, c)| {
            let mut chars: Vec<char> = pem.chars().collect();
            let at = at.index(chars.len());
            chars[at] = c;
            chars.into_iter().collect()
        });
    prop_oneof![text(), block, real, changed].boxed()
}

/// A recipient the node reads without IO: any text, but not one that names
/// another wallet at a public host, or a payment request elsewhere on this
/// one, which the node would fetch.
fn recipient() -> BoxedStrategy<String> {
    let hosts = [
        HOSTNAME,
        "NODE.example",
        "node.example:443",
        "127.0.0.1",
        "[::1]:8443",
    ];
    let paths = [
        "",
        "/",
        "/pay/stub",
        "/.well-known/pay",
        "/incoming-payments/",
        "/pay/",
    ];
    let shaped = (
        proptest::sample::select(vec!["$", "https://", "HTTPS://", "http://"]),
        proptest::sample::select(hosts.to_vec()),
        proptest::sample::select(paths.to_vec()),
        prop_oneof![2 => Just(String::new()), 1 => payment_id(), 1 => text()],
    )
        .prop_map(|(scheme, host, path, rest)| format!("{scheme}{host}{path}{rest}"));
    prop_oneof![3 => shaped, 1 => text()]
        .prop_map(|input| {
            if stays_here(&input) {
                input
            } else {
                format!("${}/pay/stub", PRIVATE_HOSTS[0])
            }
        })
        .boxed()
}

/// Whether the node reads `input` as a recipient without IO: it is none,
/// one of this node's, or at a host the outbound client refuses before it
/// connects.
fn stays_here(input: &str) -> bool {
    let Ok(recipient) = Recipient::parse(input) else {
        return true;
    };
    let url = recipient.url();
    if let Some(path) = Urls::from_origin(&format!("https://{HOSTNAME}")).own_path(url) {
        return matches!(recipient, Recipient::WalletAddress(_))
            || path.starts_with("/incoming-payments/");
    }
    url.strip_prefix("https://")
        .and_then(|rest| rest.split(['/', '?', '#']).next())
        .is_none_or(|host| host.is_empty() || host.contains('@') || PRIVATE_HOSTS.contains(&host))
}

fn cap() -> BoxedStrategy<Option<proto::SpendingCap>> {
    proptest::option::of(
        (number(), number())
            .prop_map(|(amount, window_ms)| proto::SpendingCap { amount, window_ms }),
    )
    .boxed()
}

/// Names an owner may register, or not: reserved, too short, with a
/// hyphen where a name has none.
const NAMES: &[&str] = &["alice", "www", "ab", "-alice", "xn--alice", ""];

/// The methods without fields.
fn bare() -> BoxedStrategy<Method> {
    proptest::sample::select(vec![
        Method::GetBasicState(proto::GetBasicStateRequest {}),
        Method::GetStatus(proto::GetStatusRequest {}),
        Method::GetConfig(proto::GetConfigRequest {}),
        Method::GetSetupUrl(proto::GetSetupUrlRequest {}),
        Method::ListSessions(proto::ListSessionsRequest {}),
        Method::StartPairing(proto::StartPairingRequest {}),
        Method::ListDevices(proto::ListDevicesRequest {}),
        Method::ListLedgers(proto::ListLedgersRequest {}),
        Method::ListSettlementSchemes(proto::ListSettlementSchemesRequest {}),
        Method::SubscribeLedgers(proto::SubscribeLedgersRequest {}),
        Method::ListBtpTokens(proto::ListBtpTokensRequest {}),
        Method::ListWalletAddresses(proto::ListWalletAddressesRequest {}),
        Method::ListAccounts(proto::ListAccountsRequest {}),
        Method::SubscribePackets(proto::SubscribePacketsRequest {}),
        Method::ListNodes(proto::ListNodesRequest {}),
        Method::ListPeers(proto::ListPeersRequest {}),
        Method::ListRoutes(proto::ListRoutesRequest {}),
        Method::GetRegistrationStatus(proto::GetRegistrationStatusRequest {}),
        Method::ListBans(proto::ListBansRequest {}),
        Method::ListSettlementChannels(proto::ListSettlementChannelsRequest {}),
        Method::SubscribeNodes(proto::SubscribeNodesRequest {}),
        Method::SubscribeRoutes(proto::SubscribeRoutesRequest {}),
        Method::SubscribeRegistration(proto::SubscribeRegistrationRequest {}),
        Method::SubscribeLogs(proto::SubscribeLogsRequest {}),
        Method::ListDatabaseTables(proto::ListDatabaseTablesRequest {}),
        Method::ListServices(proto::ListServicesRequest {}),
        Method::ListGrants(proto::ListGrantsRequest {}),
        Method::SubscribeGrants(proto::SubscribeGrantsRequest {}),
        Method::SubscribeNames(proto::SubscribeNamesRequest {}),
        Method::GetPeerSettings(proto::GetPeerSettingsRequest {}),
        Method::GetRateSources(proto::GetRateSourcesRequest {}),
    ])
    .boxed()
}

/// The node's settings, its certificate and the sessions.
fn settings() -> BoxedStrategy<Method> {
    prop_oneof![
        hostname().prop_map(|hostname| Method::SetHostname(proto::SetHostnameRequest { hostname })),
        prop_oneof![Just(0), Just(1), Just(2), any::<i32>()]
            .prop_map(|realm| Method::SetRealm(proto::SetRealmRequest { realm })),
        (small(), small()).prop_map(|(http_port, https_port)| {
            Method::SetPorts(proto::SetPortsRequest {
                http_port,
                https_port,
            })
        }),
        field(&["Dassie", "a", ""])
            .prop_map(|alias| Method::SetAlias(proto::SetAliasRequest { alias })),
        (pem(), pem()).prop_map(|(certificate_chain_pem, private_key_pem)| {
            Method::SetCertificate(proto::SetCertificateRequest {
                certificate_chain_pem,
                private_key_pem,
            })
        }),
        refused_directory().prop_map(|directory_url| {
            Method::GetAcmeTerms(proto::GetAcmeTermsRequest { directory_url })
        }),
        (text(), any::<bool>(), refused_directory()).prop_map(|(email, agree, directory_url)| {
            Method::ObtainCertificate(proto::ObtainCertificateRequest {
                email,
                agree_to_terms_of_service: agree,
                directory_url,
            })
        }),
        prop_oneof!["[0-9a-fA-F]{0,70}", text()]
            .prop_map(|id| Method::RevokeSession(proto::RevokeSessionRequest { id })),
        any::<bool>().prop_map(|keep_current| {
            Method::RevokeAllSessions(proto::RevokeAllSessionsRequest { keep_current })
        }),
    ]
    .boxed()
}

/// The ledgers, the BTP tokens and the debug views.
fn ledgers() -> BoxedStrategy<Method> {
    let tables = field(&[
        "sessions",
        "node_key",
        "ledger_transfers",
        "sqlite_master",
        "x; DROP TABLE x",
    ]);
    prop_oneof![
        (scheme_id(), field(&["", "{}", " {} ", "{\"a\":1}", "null"])).prop_map(|(id, config)| {
            Method::AddSettlementScheme(proto::AddSettlementSchemeRequest { id, config })
        }),
        scheme_id().prop_map(|id| {
            Method::RemoveSettlementScheme(proto::RemoveSettlementSchemeRequest { id })
        }),
        number().prop_map(|amount| Method::StubDeposit(proto::StubDepositRequest { amount })),
        scheme_id().prop_map(|settlement_scheme| {
            Method::AskFaucet(proto::AskFaucetRequest { settlement_scheme })
        }),
        (text(), field(LEDGERS), cap()).prop_map(|(label, ledger, cap)| {
            Method::CreateBtpToken(proto::CreateBtpTokenRequest { label, ledger, cap })
        }),
        number().prop_map(|id| Method::RevokeBtpToken(proto::RevokeBtpTokenRequest { id })),
        (number(), cap())
            .prop_map(|(id, cap)| Method::SetBtpTokenCap(proto::SetBtpTokenCapRequest { id, cap })),
        (
            proptest::option::of(number()),
            small(),
            proptest::option::of(small())
        )
            .prop_map(|(after_sequence, limit, account)| {
                Method::ListTransfers(proto::ListTransfersRequest {
                    after_sequence,
                    limit,
                    account,
                })
            }),
        (proptest::option::of((number(), number())), small()).prop_map(|(after, limit)| {
            Method::ListPackets(proto::ListPacketsRequest {
                after: after.map(|(epoch, sequence)| proto::TraceId { epoch, sequence }),
                limit,
            })
        }),
        (proptest::option::of(small()), small()).prop_map(|(account, history)| {
            Method::SubscribeTransfers(proto::SubscribeTransfersRequest { account, history })
        }),
        (tables, number(), small()).prop_map(|(table, cursor, limit)| {
            Method::ListDatabaseRows(proto::ListDatabaseRowsRequest {
                table,
                cursor,
                limit,
            })
        }),
        hub(),
    ]
    .boxed()
}

/// HubMemberService: refused without a hub scheme, never a bug.
fn hub() -> BoxedStrategy<Method> {
    prop_oneof![
        Just(Method::GetHubStatus(proto::GetHubStatusRequest {})),
        number().prop_map(|amount| Method::HubDeposit(proto::HubDepositRequest { amount })),
        number().prop_map(|amount| Method::HubWithdraw(proto::HubWithdrawRequest { amount })),
        any::<bool>().prop_map(|automatic| {
            Method::SetHubAutomatic(proto::SetHubAutomaticRequest { automatic })
        }),
        Just(Method::HubGiveUp(proto::HubGiveUpRequest {})),
        (
            field(&["", "{}", "{\"fees\":{\"operation\":0}}", "["]),
            number()
        )
            .prop_map(|(terms, drops)| {
                let invite = |node: &str| proto::HubFoundingInvite {
                    node: node.into(),
                    endpoint: "v.example:443".into(),
                };
                Method::FoundHub(proto::FoundHubRequest {
                    validators: vec![invite("dA"), invite("dB"), invite("dC"), invite("dD")],
                    terms,
                    escrow_funding_drops: Some(drops),
                    xrpl_server: String::new(),
                })
            }),
        field(&["", "00", "x; DROP TABLE x"])
            .prop_map(|nonce| { Method::DropHubFounding(proto::DropHubFoundingRequest { nonce }) }),
        Just(Method::ListHubFoundings(proto::ListHubFoundingsRequest {})),
        Just(Method::GetHubPayout(proto::GetHubPayoutRequest {})),
    ]
    .boxed()
}

/// Open Payments: the wallet addresses, payments and grants.
fn payments() -> BoxedStrategy<Method> {
    prop_oneof![
        field(LEDGERS)
            .prop_map(|ledger| Method::SetDefaultLedger(proto::SetDefaultLedgerRequest { ledger })),
        (
            field(LEDGERS),
            proptest::option::of(number()),
            number(),
            text(),
            proptest::option::of(node_id())
        )
            .prop_map(
                |(ledger, incoming_amount, expires_at_ms, description, uplink)| {
                    Method::CreateIncomingPayment(proto::CreateIncomingPaymentRequest {
                        ledger,
                        incoming_amount,
                        expires_at_ms,
                        description,
                        uplink,
                    })
                }
            ),
        payment_id().prop_map(|id| {
            Method::CompleteIncomingPayment(proto::CompleteIncomingPaymentRequest { id })
        }),
        small().prop_map(|limit| {
            Method::ListIncomingPayments(proto::ListIncomingPaymentsRequest { limit })
        }),
        small().prop_map(|limit| {
            Method::SubscribeIncomingPayments(proto::SubscribeIncomingPaymentsRequest { limit })
        }),
        small().prop_map(|limit| {
            Method::ListOutgoingPayments(proto::ListOutgoingPaymentsRequest { limit })
        }),
        small().prop_map(|limit| {
            Method::SubscribeOutgoingPayments(proto::SubscribeOutgoingPaymentsRequest { limit })
        }),
        (payment_id(), text()).prop_map(|(id, key)| Method::GetGrantRequest(
            proto::GetGrantRequestRequest { id, key }
        )),
        (payment_id(), text(), any::<bool>()).prop_map(|(id, key, approve)| {
            Method::DecideGrant(proto::DecideGrantRequest { id, key, approve })
        }),
        payment_id().prop_map(|id| Method::RevokeGrant(proto::RevokeGrantRequest { id })),
        (text(), text()).prop_map(|(secret, name)| Method::ClaimPairing(
            proto::ClaimPairingRequest { secret, name }
        )),
        text().prop_map(|id| Method::RevokeDevice(proto::RevokeDeviceRequest { id })),
    ]
    .boxed()
}

/// Send, and the network.
fn sending() -> BoxedStrategy<Method> {
    use proto::quote_send_request::Amount;
    let amount = proptest::option::of(prop_oneof![
        number().prop_map(Amount::DebitAmount),
        number().prop_map(Amount::ReceiveAmount),
    ]);
    let rate_sources = proptest::collection::vec(
        (
            field(&["coinbase", "kraken", "bitstamp", "ecb"]),
            field(&["", "https://rates.example", "https://127.0.0.1:8443/x"]),
        ),
        0..10,
    );
    prop_oneof![
        recipient().prop_map(|recipient| {
            Method::ResolveRecipient(proto::ResolveRecipientRequest { recipient })
        }),
        (recipient(), field(LEDGERS), amount, text()).prop_map(
            |(recipient, ledger, amount, note)| {
                Method::QuoteSend(proto::QuoteSendRequest {
                    recipient,
                    ledger,
                    amount,
                    note,
                    credentials: None,
                })
            }
        ),
        payment_id()
            .prop_map(|quote_id| Method::SendPayment(proto::SendPaymentRequest { quote_id })),
        payment_id().prop_map(|id| {
            Method::SubscribeOutgoingPayment(proto::SubscribeOutgoingPaymentRequest { id })
        }),
        node_id().prop_map(|node| {
            Method::RegisterWithBootstrap(proto::RegisterWithBootstrapRequest { node })
        }),
        node_id()
            .prop_map(|node| Method::TerminatePeering(proto::TerminatePeeringRequest { node })),
        node_id().prop_map(|node| Method::BanNode(proto::BanNodeRequest { node })),
        node_id().prop_map(|node| Method::UnbanNode(proto::UnbanNodeRequest { node })),
        field(NAMES).prop_map(|name| Method::RegisterName(proto::RegisterNameRequest { name })),
        field(NAMES).prop_map(|name| Method::RenewName(proto::RenewNameRequest { name })),
        proptest::option::of((small(), small(), proptest::option::of(small()))).prop_map(
            |settings| {
                Method::SetPeerSettings(proto::SetPeerSettingsRequest {
                    settings: settings.map(|(connector_fee_ppm, fx_spread_ppm, rebalance_ppm)| {
                        proto::PeerSettings {
                            connector_fee_ppm,
                            fx_spread_ppm,
                            rebalance_ppm,
                        }
                    }),
                })
            }
        ),
        rate_sources.prop_map(|sources| {
            Method::SetRateSources(proto::SetRateSourcesRequest {
                sources: sources
                    .into_iter()
                    .map(|(kind, url)| proto::RateSource { kind, url })
                    .collect(),
            })
        }),
        field(&["USD", "EUR", "XRP", ""]).prop_map(|currency| {
            Method::SetDisplayCurrency(proto::SetDisplayCurrencyRequest { currency })
        }),
    ]
    .boxed()
}

/// Any method but those that start network IO, with near-valid or
/// arbitrary fields.
fn method() -> BoxedStrategy<Method> {
    prop_oneof![bare(), settings(), ledgers(), payments(), sending()].boxed()
}

proptest! {
    // Each case sets a node up and starts its engine.
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn owner_calls(methods in proptest::collection::vec(method(), 1..8)) {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let shutdown = CancellationToken::new();
        let (node, engine) = running_node(&runtime, &shutdown);
        let calls: Vec<(u32, Method)> = (0..).zip(methods).collect();
        let (closed, messages) = serve_owner(&node, &runtime, &calls);
        prop_assert!(matches!(closed, Closed::ByClient | Closed::ProtocolError), "{:?}", closed);
        prop_assert!(answered(&expected(&calls), &messages), "unanswered: {:?}", messages);
        for message in &messages {
            if let Some(server_message::Kind::Error(error)) = &message.kind {
                prop_assert_ne!(error.code(), proto::ErrorCode::Internal, "{:?}", error);
            }
        }
        shutdown.cancel();
        // The engine ran to the end, without a panic.
        runtime.block_on(engine).unwrap();
    }
}
