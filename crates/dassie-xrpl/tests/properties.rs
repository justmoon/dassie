//! Nothing that comes from the network, or from disk, makes the crate
//! panic or breaks the scheme's invariants
//! (spec/areas/testing.md#property-tests, spec/decisions/tigerstyle.md): the
//! ledger's messages, peers' bytes, the config and the state the books kept.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::too_many_lines
)]

use dassie_ledger::{MAX_TOTAL, Name};
use dassie_node_core::testing::SeededRng;
use dassie_settlement::{Effect, LedgerInput, LedgerStatus, SettlementScheme};
use dassie_xrpl::amount::{MAX_DECIMALS, MAX_EXPONENT, MAX_MANTISSA, MIN_EXPONENT, MIN_MANTISSA};
use dassie_xrpl::api::line_flags;
use dassie_xrpl::codec::{
    self, Amount, Currency, MAX_MEMO_BYTES, MAX_MEMOS, Memo, Payment, TF_PARTIAL_PAYMENT,
    TokenAmount, TokenValue, Transaction, TrustSet,
};
use dassie_xrpl::scheme::{Config, MEMO_TYPE};
use dassie_xrpl::{
    AccountId, MAX_DROPS, UNITS_PER_DROP, Wallet, XrplScheme, api, base58, from_hex, hex_upper,
};
use proptest::prelude::*;
use serde_json::{Map, Value, json};

const ENTROPY: [u8; 32] = [5; 32];

fn node() -> Name {
    Name::new("node").unwrap()
}

/// The peer the scheme knows, and settles with.
fn peer() -> Name {
    Name::new("peer").unwrap()
}

fn ours() -> AccountId {
    Wallet::from_entropy(&ENTROPY).account()
}

fn theirs() -> AccountId {
    Wallet::from_entropy(&[7; 32]).account()
}

fn scheme() -> XrplScheme {
    XrplScheme::new("", &ENTROPY, node(), Some(1_000), &[]).unwrap()
}

/// Peering info as a scheme writes it: a length byte, then the address.
fn address_bytes(account: AccountId) -> Vec<u8> {
    let address = account.to_address();
    let mut out = vec![u8::try_from(address.len()).unwrap()];
    out.extend_from_slice(address.as_bytes());
    out
}

/// A change to valid bytes.
#[derive(Clone, Debug)]
enum Mutation {
    /// A byte changed, by a non-zero XOR.
    Flip(usize, u8),
    /// Cut short.
    Cut(usize),
    /// Bytes put in.
    Insert(usize, Vec<u8>),
    /// Bytes taken out.
    Remove(usize, usize),
}

fn mutations() -> impl Strategy<Value = Vec<Mutation>> {
    let mutation = prop_oneof![
        (any::<usize>(), 1..=u8::MAX).prop_map(|(at, mask)| Mutation::Flip(at, mask)),
        any::<usize>().prop_map(Mutation::Cut),
        (any::<usize>(), prop::collection::vec(any::<u8>(), 1..8))
            .prop_map(|(at, bytes)| Mutation::Insert(at, bytes)),
        (any::<usize>(), 1usize..8).prop_map(|(at, count)| Mutation::Remove(at, count)),
    ];
    prop::collection::vec(mutation, 0..4)
}

fn mutate(mut bytes: Vec<u8>, mutations: &[Mutation]) -> Vec<u8> {
    for mutation in mutations {
        let length = bytes.len();
        match mutation {
            Mutation::Flip(at, mask) => {
                if length > 0 {
                    bytes[at % length] ^= mask;
                }
            }
            Mutation::Cut(at) => bytes.truncate(at % (length + 1)),
            Mutation::Insert(at, more) => {
                let at = at % (length + 1);
                bytes.splice(at..at, more.iter().copied());
            }
            Mutation::Remove(at, count) => {
                let at = at % (length + 1);
                bytes.drain(at..(at + count).min(length));
            }
        }
    }
    bytes
}

/// A Payment the codec can write: any fields within the ledger's bounds.
fn payment() -> impl Strategy<Value = Payment> {
    (
        (any::<[u8; 20]>(), any::<[u8; 20]>()),
        (
            prop_oneof![
                (0..=MAX_DROPS).prop_map(Amount::Xrp),
                token_amount().prop_map(|token| Amount::Token(TokenAmount {
                    value: positive(token.value),
                    ..token
                })),
            ],
            0..=MAX_DROPS,
        ),
        (
            any::<u32>(),
            any::<u32>(),
            any::<u32>().prop_map(|flags| flags & !TF_PARTIAL_PAYMENT),
            any::<Option<u32>>(),
        ),
        prop::collection::vec(memo(), 0..=MAX_MEMOS),
        (any::<[u8; 33]>(), any::<Option<[u8; 64]>>()),
    )
        .prop_map(
            |(
                (account, destination),
                (amount, fee),
                (sequence, last_ledger_sequence, flags, network_id),
                memos,
                (signing_pub_key, signature),
            )| Payment {
                account: AccountId(account),
                destination: AccountId(destination),
                amount,
                fee,
                sequence,
                last_ledger_sequence,
                flags,
                network_id,
                memos,
                signing_pub_key,
                signature,
            },
        )
}

fn memo() -> impl Strategy<Value = Memo> {
    let bytes = |max| prop::collection::vec(any::<u8>(), 0..=max);
    (bytes(MAX_MEMO_BYTES), bytes(MAX_MEMO_BYTES)).prop_map(|(memo_type, memo_data)| Memo {
        memo_type,
        memo_data,
    })
}

/// A TrustSet the codec can write: any fields within the ledger's bounds.
fn trust_set() -> impl Strategy<Value = TrustSet> {
    (
        any::<[u8; 20]>(),
        token_amount(),
        0..=MAX_DROPS,
        (
            any::<u32>(),
            any::<u32>(),
            any::<u32>(),
            any::<Option<u32>>(),
        ),
        prop::collection::vec(memo(), 0..=MAX_MEMOS),
        (any::<[u8; 33]>(), any::<Option<[u8; 64]>>()),
    )
        .prop_map(
            |(
                account,
                limit,
                fee,
                (sequence, last_ledger_sequence, flags, network_id),
                memos,
                (signing_pub_key, signature),
            )| TrustSet {
                account: AccountId(account),
                limit: TokenAmount {
                    value: positive(limit.value),
                    ..limit
                },
                fee,
                sequence,
                last_ledger_sequence,
                flags,
                network_id,
                memos,
                signing_pub_key,
                signature,
            },
        )
}

/// The value, or its negation if it is below zero.
fn positive(value: TokenValue) -> TokenValue {
    if value.is_negative() {
        value.negate()
    } else {
        value
    }
}

/// RLUSD's currency code.
const RLUSD: &str = "524C555344000000000000000000000000000000";

/// Any value the ledger can express: its edges, any mantissa and
/// exponent, and amounts near the books' units.
fn token_value() -> impl Strategy<Value = TokenValue> {
    prop_oneof![
        1 => Just(TokenValue::ZERO),
        1 => Just(TokenValue::MAX),
        1 => Just(TokenValue::MAX.negate()),
        1 => any::<bool>().prop_map(|negative| TokenValue::new(negative, 1, MIN_EXPONENT + 15).unwrap()),
        4 => (any::<bool>(), MIN_MANTISSA..=MAX_MANTISSA, MIN_EXPONENT..=MAX_EXPONENT)
            .prop_map(|(negative, mantissa, exponent)| TokenValue::new(negative, mantissa, exponent).unwrap()),
        4 => (any::<bool>(), 0..=MAX_MANTISSA, -40i32..=10)
            .prop_filter_map("exact", |(negative, mantissa, exponent)| TokenValue::new(negative, mantissa, exponent)),
    ]
}

fn currency() -> impl Strategy<Value = Currency> {
    prop_oneof![
        2 => Just(Currency::parse(RLUSD).unwrap()),
        1 => Just(Currency::parse("USD").unwrap()),
        1 => any::<[u8; 20]>().prop_map(Currency).prop_filter("a token's", Currency::is_token),
    ]
}

fn token_amount() -> impl Strategy<Value = TokenAmount> {
    (token_value(), currency(), any::<[u8; 20]>()).prop_filter_map(
        "a token",
        |(value, currency, issuer)| {
            let amount = TokenAmount {
                value,
                currency,
                issuer: AccountId(issuer),
            };
            amount.is_valid().then_some(amount)
        },
    )
}

/// Two accounts that are not one.
fn two_accounts() -> impl Strategy<Value = (AccountId, AccountId)> {
    (any::<[u8; 20]>(), any::<[u8; 20]>())
        .prop_filter("two accounts", |(a, b)| a != b)
        .prop_map(|(a, b)| (AccountId(a), AccountId(b)))
}

/// A token amount as the ledger writes it.
fn amount_json(value: TokenValue, currency: Currency, issuer: &str) -> Value {
    json!({ "currency": currency.to_string(), "issuer": issuer, "value": value.to_string() })
}

/// A `RippleState` between `a` and `b`, from `a`'s side: its balance and
/// limit, `b`'s limit, and the flags `a` and `b` set, each as the low
/// side's ([`side_flags`]), put on the low or the high side by which of
/// the two is the lower number.
fn ripple_state(
    (a, b): (AccountId, AccountId),
    currency: Currency,
    balance: TokenValue,
    (limit_a, limit_b): (TokenValue, TokenValue),
    (flags_a, flags_b): (u32, u32),
) -> Value {
    let a_is_low = a < b;
    let (low, high, balance, low_limit, high_limit) = if a_is_low {
        (a, b, balance, limit_a, limit_b)
    } else {
        (b, a, balance.negate(), limit_b, limit_a)
    };
    let (low_flags, high_flags) = if a_is_low {
        (flags_a, flags_b)
    } else {
        (flags_b, flags_a)
    };
    json!({
        "LedgerEntryType": "RippleState",
        // Each of the high side's flags is the low side's, doubled.
        "Flags": low_flags | high_flags << 1,
        "Balance": amount_json(balance, currency, "rrrrrrrrrrrrrrrrrrrrBZbvji"),
        "LowLimit": amount_json(low_limit, currency, &low.to_address()),
        "HighLimit": amount_json(high_limit, currency, &high.to_address()),
        "HighNode": "0",
        "LowNode": "0",
    })
}

/// One side's flags from five bits (reserve, auth, no ripple, freeze,
/// deep freeze), as the low side's.
fn side_flags(bits: u32) -> u32 {
    let flag = |bit: u32, low: u32| if bits & (1 << bit) != 0 { low } else { 0 };
    flag(0, line_flags::LOW_RESERVE)
        | flag(1, line_flags::LOW_AUTH)
        | flag(2, line_flags::LOW_NO_RIPPLE)
        | flag(3, line_flags::LOW_FREEZE)
        | flag(4, line_flags::LOW_DEEP_FREEZE)
}

/// What `bits` say one side set.
fn expected_flags(bits: u32) -> api::LineFlags {
    api::LineFlags {
        reserve: bits & 1 != 0,
        auth: bits & 2 != 0,
        no_ripple: bits & 4 != 0,
        freeze: bits & 8 != 0,
        deep_freeze: bits & 16 != 0,
    }
}

/// A validated transaction by `account` whose metadata has `nodes`.
fn tx_with(account: AccountId, nodes: Vec<Value>, delivered: Option<Value>) -> Value {
    let mut meta =
        json!({ "TransactionIndex": 0, "TransactionResult": "tesSUCCESS", "AffectedNodes": nodes });
    if let Some(delivered) = delivered {
        meta["delivered_amount"] = delivered;
    }
    json!({
        "TransactionType": "Payment",
        "Account": account.to_address(),
        "Sequence": 1,
        "Fee": "12",
        "hash": "AB".repeat(32),
        "ledger_index": 5,
        "validated": true,
        "meta": meta,
    })
}

/// Config text: anything, JSON objects of the right keys and wrong ones,
/// and those changed.
fn config_text() -> impl Strategy<Value = String> {
    let url = (
        prop_oneof![
            Just("wss://"),
            Just("ws://"),
            Just("https://"),
            Just("http://"),
            Just("")
        ],
        "\\PC{0,40}",
    )
        .prop_map(|(scheme, rest)| Value::from(format!("{scheme}{rest}")));
    let value = prop_oneof![
        4 => url,
        1 => Just(Value::Null),
        1 => any::<i64>().prop_map(Value::from),
        1 => Just(json!([])),
        1 => Just(json!({ "server": "wss://a" })),
    ];
    let key = prop_oneof![
        3 => Just("server".to_string()),
        3 => Just("faucet".to_string()),
        1 => "\\PC{0,8}",
    ];
    let object = prop::collection::vec((key, value), 0..4)
        .prop_map(|fields| Value::Object(fields.into_iter().collect::<Map<_, _>>()).to_string());
    prop_oneof![
        "\\PC{0,80}",
        "\\s{0,3}",
        (object.clone(), "\\s{0,3}").prop_map(|(text, space)| format!("{space}{text}{space}")),
        (object, mutations()).prop_map(|(text, mutations)| {
            String::from_utf8_lossy(&mutate(text.into_bytes(), &mutations)).into_owned()
        }),
        // Past what a config may hold.
        ("\\PC{0,8}", 1_000usize..1_100)
            .prop_map(|(text, pad)| format!(r#"{{"server":"wss://{}{text}"}}"#, "a".repeat(pad))),
    ]
}

/// What a message is built against: the hashes the scheme knows, and the
/// ledger the network is at.
#[derive(Clone, Copy)]
struct Context<'a> {
    hashes: &'a [[u8; 32]],
    ledger: u64,
}

/// For values that name neither.
const NONE: Context<'static> = Context {
    hashes: &[],
    ledger: 0,
};

/// A JSON value in what comes from outside, built when the message is:
/// mostly the kind its field holds, sometimes anything.
#[derive(Clone, Debug)]
enum Leaf {
    /// The field left out.
    Absent,
    Null,
    Bool(bool),
    Number(i64),
    Float(f64),
    Text(String),
    Object(Vec<(String, i64)>),
    Array(Vec<i64>),
    /// A ledger index, sequence or fee, as a JSON number.
    Index(u64),
    /// A ledger index this far from the network's.
    Ledger(i64),
    /// XRP, as the ledger writes it: a decimal string.
    Drops(u64),
    /// Our account's address.
    Ours,
    /// The known peer's.
    Theirs,
    Account([u8; 20]),
    /// A hash the scheme knows, by its place in what it knows.
    Hash(usize),
    Hex(Vec<u8>),
    Code(String),
}

impl Leaf {
    fn value(&self, context: Context<'_>) -> Option<Value> {
        Some(match self {
            Self::Absent => return None,
            Self::Null => Value::Null,
            Self::Bool(value) => Value::from(*value),
            Self::Number(value) => Value::from(*value),
            Self::Float(value) => {
                serde_json::Number::from_f64(*value).map_or(Value::Null, Value::Number)
            }
            Self::Text(text) | Self::Code(text) => Value::from(text.as_str()),
            Self::Object(fields) => Value::Object(
                fields
                    .iter()
                    .map(|(key, value)| (key.clone(), Value::from(*value)))
                    .collect(),
            ),
            Self::Array(values) => Value::from(values.clone()),
            Self::Index(value) => Value::from(*value),
            Self::Ledger(offset) => Value::from(context.ledger.saturating_add_signed(*offset)),
            Self::Drops(drops) => Value::from(drops.to_string()),
            Self::Ours => Value::from(ours().to_address()),
            Self::Theirs => Value::from(theirs().to_address()),
            Self::Account(id) => Value::from(AccountId(*id).to_address()),
            Self::Hash(at) => Value::from(hex_upper(
                context
                    .hashes
                    .get(at % context.hashes.len().max(1))
                    .unwrap_or(&[0; 32]),
            )),
            Self::Hex(bytes) => Value::from(hex_upper(bytes)),
        })
    }
}

/// An object of the fields present.
fn object(context: Context<'_>, fields: &[(&str, &Leaf)]) -> Value {
    Value::Object(
        fields
            .iter()
            .filter_map(|(key, leaf)| Some(((*key).to_string(), leaf.value(context)?)))
            .collect(),
    )
}

/// `value` with more fields: nested ones already built, or another
/// object's.
fn with<K: Into<String>>(mut value: Value, fields: impl IntoIterator<Item = (K, Value)>) -> Value {
    if let Some(object) = value.as_object_mut() {
        for (key, field) in fields {
            object.insert(key.into(), field);
        }
    }
    value
}

/// An object's fields.
fn fields(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(fields) => fields,
        _ => Map::new(),
    }
}

fn junk() -> BoxedStrategy<Leaf> {
    prop_oneof![
        Just(Leaf::Absent),
        Just(Leaf::Null),
        any::<bool>().prop_map(Leaf::Bool),
        any::<i64>().prop_map(Leaf::Number),
        any::<f64>().prop_map(Leaf::Float),
        "\\PC{0,20}".prop_map(Leaf::Text),
        prop::collection::vec(("[A-Za-z]{1,12}", any::<i64>()), 0..3).prop_map(Leaf::Object),
        prop::collection::vec(any::<i64>(), 0..3).prop_map(Leaf::Array),
        any::<u64>().prop_map(Leaf::Index),
        any::<u64>().prop_map(Leaf::Drops),
        prop::collection::vec(any::<u8>(), 0..40).prop_map(Leaf::Hex),
    ]
    .boxed()
}

/// A field that mostly holds what `plausible` makes.
fn field(plausible: impl Strategy<Value = Leaf> + 'static) -> BoxedStrategy<Leaf> {
    prop_oneof![40 => plausible, 1 => junk()].boxed()
}

/// A ledger index or sequence: mostly near the ledgers the scheme sees.
fn index() -> BoxedStrategy<Leaf> {
    field(prop_oneof![4 => 0u64..64, 1 => any::<u32>().prop_map(u64::from)].prop_map(Leaf::Index))
}

/// A ledger index: mostly near the network's, so the ledger moves on.
fn ledger() -> BoxedStrategy<Leaf> {
    field(prop_oneof![
        6 => (-4i64..4).prop_map(Leaf::Ledger),
        2 => (0u64..64).prop_map(Leaf::Index),
        1 => any::<u32>().prop_map(|index| Leaf::Index(index.into())),
    ])
}

/// A validated ledger's index and close time: mostly closed in the order of
/// its index, sometimes at any time, as on a chain started again.
fn validated_ledger() -> BoxedStrategy<(Leaf, Leaf)> {
    (
        ledger(),
        prop_oneof![8 => Just(None), 1 => ledger().prop_map(Some)],
    )
        .prop_map(|(index, close_time)| {
            let close_time = close_time.unwrap_or_else(|| index.clone());
            (index, close_time)
        })
        .boxed()
}

/// A reference fee, as a number of drops: past what the scheme pays, it
/// waits, so mostly not.
fn fee() -> BoxedStrategy<Leaf> {
    field(
        prop_oneof![12 => 0u64..20, 1 => 0u64..2_000_000, 1 => any::<u64>()].prop_map(Leaf::Index),
    )
}

/// A reserve, as a number of drops.
fn reserve() -> BoxedStrategy<Leaf> {
    field(prop_oneof![12 => 0u64..2_000_000, 1 => any::<u64>()].prop_map(Leaf::Index))
}

fn drops() -> BoxedStrategy<Leaf> {
    field(
        prop_oneof![
            // Past what the books hold stops the scheme: rarely.
            30 => 0u64..10_000_000_000,
            1 => 0..=MAX_DROPS,
            1 => any::<u64>(),
        ]
        .prop_map(Leaf::Drops),
    )
}

/// An account: ours, the peer's or another, by weight.
fn account(ours: u32, theirs: u32) -> BoxedStrategy<Leaf> {
    field(prop_oneof![
        ours => Just(Leaf::Ours),
        theirs => Just(Leaf::Theirs),
        1 => any::<[u8; 20]>().prop_map(Leaf::Account),
    ])
}

fn hash() -> BoxedStrategy<Leaf> {
    field(prop_oneof![
        4 => any::<usize>().prop_map(Leaf::Hash),
        1 => any::<[u8; 32]>().prop_map(|hash| Leaf::Hex(hash.to_vec())),
    ])
}

fn code(codes: &'static [&'static str]) -> BoxedStrategy<Leaf> {
    field(prop_oneof![
        4 => prop::sample::select(codes).prop_map(|code| Leaf::Code(code.to_string())),
        1 => "[a-z]{3}[A-Z_]{0,12}".prop_map(Leaf::Code),
    ])
}

const RESULTS: &[&str] = &[
    "tesSUCCESS",
    "tesSUCCESS",
    "tesSUCCESS",
    "tesSUCCESS",
    "tecUNFUNDED_PAYMENT",
    "temBAD_FEE",
    "terQUEUED",
    "telINSUF_FEE_P",
    "tefPAST_SEQ",
];

const ERRORS: &[&str] = &[
    "actNotFound",
    "txnNotFound",
    "txnNotFound",
    "lgrNotFound",
    "lgrIdxsInvalid",
    "tooBusy",
    "noNetwork",
];

/// What a transaction did to one ledger entry.
#[derive(Clone, Debug)]
struct Node {
    kind: Leaf,
    entry: Leaf,
    account: Leaf,
    balance: Leaf,
    previous: Leaf,
    owner_count: Leaf,
}

fn affected_node() -> impl Strategy<Value = Node> {
    let kind = code(&["CreatedNode", "ModifiedNode", "ModifiedNode", "DeletedNode"]);
    let entry = code(&["AccountRoot", "AccountRoot", "AccountRoot", "RippleState"]);
    (
        kind,
        entry,
        account(4, 1),
        drops(),
        prop_oneof![1 => Just(Leaf::Absent), 3 => drops()],
        index(),
    )
        .prop_map(
            |(kind, entry, account, balance, previous, owner_count)| Node {
                kind,
                entry,
                account,
                balance,
                previous,
                owner_count,
            },
        )
}

impl Node {
    fn value(&self, context: Context<'_>) -> Value {
        let kind = match &self.kind {
            Leaf::Code(kind) => kind.clone(),
            other => other
                .value(context)
                .map_or_else(String::new, |value| value.to_string()),
        };
        let fields = object(
            context,
            &[
                ("Account", &self.account),
                ("Balance", &self.balance),
                ("OwnerCount", &self.owner_count),
            ],
        );
        let fields_key = if kind == "CreatedNode" {
            "NewFields"
        } else {
            "FinalFields"
        };
        let inner = with(
            object(context, &[("LedgerEntryType", &self.entry)]),
            vec![
                (fields_key, fields),
                (
                    "PreviousFields",
                    object(context, &[("Balance", &self.previous)]),
                ),
            ],
        );
        Value::Object(Map::from_iter([(kind, inner)]))
    }
}

/// A transaction as the ledger shows it, with its metadata.
#[derive(Clone, Debug)]
struct Tx {
    kind: Leaf,
    account: Leaf,
    destination: Leaf,
    fee: Leaf,
    sequence: Leaf,
    hash: Leaf,
    memo_type: Leaf,
    memo_data: Leaf,
    ledger_index: Leaf,
    validated: Leaf,
    index: Leaf,
    result: Leaf,
    delivered: Leaf,
    nodes: Vec<Node>,
}

fn transaction() -> impl Strategy<Value = Tx> {
    let memo_type = field(prop_oneof![
        4 => Just(Leaf::Hex(MEMO_TYPE.to_vec())),
        1 => Just(Leaf::Absent),
    ]);
    let memo_data = field(prop_oneof![
        4 => Just(Leaf::Hex(b"peer".to_vec())),
        1 => Just(Leaf::Hex(b"node".to_vec())),
    ]);
    let validated = field(prop_oneof![6 => Just(Leaf::Bool(true)), 1 => Just(Leaf::Bool(false))]);
    (
        (
            code(&["Payment", "Payment", "Payment", "AccountSet"]),
            // Mostly from the peer to us, or from us to the peer.
            account(2, 3),
            account(3, 1),
            drops(),
            index(),
            hash(),
        ),
        (memo_type, memo_data, ledger(), validated, index()),
        (
            code(RESULTS),
            drops(),
            prop::collection::vec(affected_node(), 0..4),
        ),
        prop::option::weighted(0.3, affected_node()),
    )
        .prop_map(
            |(
                (kind, account, destination, fee, sequence, hash),
                (memo_type, memo_data, ledger_index, validated, index),
                (result, delivered, mut nodes),
                settles,
            )| {
                let mut tx = Tx {
                    kind,
                    account,
                    destination,
                    fee,
                    sequence,
                    hash,
                    memo_type,
                    memo_data,
                    ledger_index,
                    validated,
                    index,
                    result,
                    delivered,
                    nodes: Vec::new(),
                };
                // Sometimes a settlement from the peer, as it would be:
                // other fields as they come.
                if let Some(ours) = settles {
                    tx.kind = Leaf::Code("Payment".into());
                    tx.account = Leaf::Theirs;
                    tx.destination = Leaf::Ours;
                    tx.result = Leaf::Code("tesSUCCESS".into());
                    tx.memo_type = Leaf::Hex(MEMO_TYPE.to_vec());
                    tx.memo_data = Leaf::Hex(b"peer".to_vec());
                    nodes.push(Node {
                        kind: Leaf::Code("ModifiedNode".into()),
                        entry: Leaf::Code("AccountRoot".into()),
                        account: Leaf::Ours,
                        ..ours
                    });
                }
                tx.nodes = nodes;
                tx
            },
        )
}

impl Tx {
    /// The transaction's fields, and its metadata.
    fn parts(&self, context: Context<'_>) -> (Value, Value) {
        let memo = object(
            context,
            &[("MemoType", &self.memo_type), ("MemoData", &self.memo_data)],
        );
        let tx = with(
            object(
                context,
                &[
                    ("TransactionType", &self.kind),
                    ("Account", &self.account),
                    ("Destination", &self.destination),
                    ("Fee", &self.fee),
                    ("Sequence", &self.sequence),
                    ("hash", &self.hash),
                ],
            ),
            vec![("Memos", json!([{ "Memo": memo }]))],
        );
        let nodes: Vec<Value> = self.nodes.iter().map(|node| node.value(context)).collect();
        let meta = with(
            object(
                context,
                &[
                    ("TransactionIndex", &self.index),
                    ("TransactionResult", &self.result),
                    ("delivered_amount", &self.delivered),
                ],
            ),
            vec![("AffectedNodes", Value::from(nodes))],
        );
        (tx, meta)
    }

    /// As an `account_tx` entry, or a `tx` result.
    fn entry(&self, context: Context<'_>, nested: bool) -> Value {
        let (tx, meta) = self.parts(context);
        let place = object(
            context,
            &[
                ("ledger_index", &self.ledger_index),
                ("validated", &self.validated),
            ],
        );
        if nested {
            with(place, vec![("tx", tx), ("meta", meta)])
        } else {
            with(with(tx, vec![("meta", meta)]), fields(place))
        }
    }

    /// As the stream sends it.
    fn message(&self, context: Context<'_>) -> Value {
        let (tx, meta) = self.parts(context);
        with(
            object(
                context,
                &[
                    ("ledger_index", &self.ledger_index),
                    ("validated", &self.validated),
                ],
            ),
            vec![
                ("type", "transaction".into()),
                ("transaction", tx),
                ("meta", meta),
            ],
        )
    }
}

/// How an answer goes.
#[derive(Clone, Debug)]
enum Status {
    Success,
    Error {
        error: Leaf,
        searched_all: Leaf,
        /// Whether the ledger the answer names is validated.
        validated: Leaf,
    },
    /// What a lookup of a transaction the ledger never saw gets.
    NotFound {
        searched_all: Leaf,
    },
    Other(Leaf),
}

/// An answer to a request, with every result field any request reads,
/// so it is the same strategy whatever was asked.
#[derive(Clone, Debug)]
struct Answer {
    status: Status,
    /// `server_state`'s: the validated ledger, its close time, base fee and
    /// reserves, the load, and the network.
    server: [Leaf; 8],
    /// `account_info`'s: balance, sequence and owner count, and the ledger.
    info: [Leaf; 4],
    /// `account_tx`'s.
    transactions: Vec<Tx>,
    marker: Leaf,
    ledger_index_max: Leaf,
    /// `tx`'s.
    tx: Tx,
    /// `submit`'s.
    engine_result: Leaf,
}

fn answer() -> impl Strategy<Value = Answer> {
    let status = prop_oneof![
        24 => Just(Status::Success),
        4 => (
            code(ERRORS),
            field(prop_oneof![
                1 => Just(Leaf::Absent),
                1 => Just(Leaf::Bool(false)),
                2 => Just(Leaf::Bool(true)),
            ]),
            field(prop_oneof![1 => Just(Leaf::Absent), 3 => Just(Leaf::Bool(true))]),
        )
            .prop_map(|(error, searched_all, validated)| Status::Error {
                error,
                searched_all,
                validated,
            }),
        3 => field(prop_oneof![
            1 => Just(Leaf::Bool(false)),
            3 => Just(Leaf::Bool(true)),
        ])
        .prop_map(|searched_all| Status::NotFound { searched_all }),
        1 => junk().prop_map(Status::Other),
    ];
    let network = field(prop_oneof![
        // Another network stops the scheme: rarely.
        20 => Just(Leaf::Index(1)),
        1 => Just(Leaf::Index(0)),
        1 => Just(Leaf::Index(2)),
        1 => Just(Leaf::Absent),
    ]);
    let load =
        || field(prop_oneof![6 => Just(Leaf::Absent), 1 => (0u64..1_000).prop_map(Leaf::Index)]);
    let server = (
        validated_ledger(),
        fee(),
        reserve(),
        reserve(),
        load(),
        load(),
        network,
    );
    let info = (drops(), index(), index(), ledger());
    let marker = prop_oneof![4 => Just(Leaf::Absent), 1 => junk()];
    (
        status,
        server,
        info,
        prop::collection::vec(transaction(), 0..3),
        marker,
        ledger(),
        transaction(),
        code(&[
            "tesSUCCESS",
            "temBAD_FEE",
            "terQUEUED",
            "telINSUF_FEE_P",
            "tefPAST_SEQ",
        ]),
    )
        .prop_map(
            |(status, server, info, transactions, marker, ledger_index_max, tx, engine_result)| {
                Answer {
                    status,
                    server: [
                        server.0.0, server.0.1, server.1, server.2, server.3, server.4, server.5,
                        server.6,
                    ],
                    info: [info.0, info.1, info.2, info.3],
                    transactions,
                    marker,
                    ledger_index_max,
                    tx,
                    engine_result,
                }
            },
        )
}

impl Answer {
    fn message(&self, id: u64, context: Context<'_>) -> Value {
        let [
            seq,
            close_time,
            base_fee,
            reserve_base,
            reserve_inc,
            load_factor,
            load_base,
            network_id,
        ] = &self.server;
        let [balance, sequence, owner_count, ledger_index] = &self.info;
        let state = with(
            object(
                context,
                &[
                    ("load_factor", load_factor),
                    ("load_base", load_base),
                    ("network_id", network_id),
                ],
            ),
            vec![(
                "validated_ledger",
                object(
                    context,
                    &[
                        ("seq", seq),
                        ("close_time", close_time),
                        ("base_fee", base_fee),
                        ("reserve_base", reserve_base),
                        ("reserve_inc", reserve_inc),
                    ],
                ),
            )],
        );
        let account_data = object(
            context,
            &[
                ("Balance", balance),
                ("Sequence", sequence),
                ("OwnerCount", owner_count),
            ],
        );
        let transactions: Vec<Value> = self
            .transactions
            .iter()
            .map(|tx| tx.entry(context, true))
            .collect();
        // The `tx` result is the transaction itself, so the others' fields
        // go next to its own.
        let result = with(
            with(
                self.tx.entry(context, false),
                fields(object(
                    context,
                    &[
                        ("ledger_index_max", &self.ledger_index_max),
                        ("marker", &self.marker),
                        ("engine_result", &self.engine_result),
                    ],
                )),
            ),
            vec![
                ("state", state),
                ("account_data", account_data),
                ("transactions", Value::from(transactions)),
            ],
        );
        let result = match ledger_index.value(context) {
            Some(index) => with(result, vec![("ledger_index", index)]),
            None => result,
        };
        let message = json!({ "type": "response", "id": id, "result": result });
        match &self.status {
            Status::Success => with(message, vec![("status", "success".into())]),
            Status::Error {
                error,
                searched_all,
                validated,
            } => with(
                with(message, vec![("status", "error".into())]),
                fields(object(
                    context,
                    &[
                        ("error", error),
                        ("searched_all", searched_all),
                        ("validated", validated),
                    ],
                )),
            ),
            Status::NotFound { searched_all } => with(
                with(
                    message,
                    vec![("status", "error".into()), ("error", "txnNotFound".into())],
                ),
                fields(object(context, &[("searched_all", searched_all)])),
            ),
            Status::Other(status) => match status.value(context) {
                Some(status) => with(message, vec![("status", status)]),
                None => message,
            },
        }
    }
}

/// Something that happens to the scheme.
#[derive(Clone, Debug)]
enum Step {
    /// The answer to one of the requests it sent, picked by place; to an
    /// ID it never sent if there are none.
    Answer(usize, Box<Answer>),
    /// A transaction on the stream.
    Transaction(Box<Tx>),
    /// A ledger closed: its index, close time, fee and reserves.
    LedgerClosed([Leaf; 5]),
    /// Bytes, mostly no JSON at all.
    Raw(Vec<u8>),
    /// The connection is made anew.
    Connect,
    /// The connection went down, and is made again at once or not.
    Disconnect(bool),
    Funded(bool),
    /// Time passes.
    Timer(u64),
    /// A settlement of drops to the peer, executed or discarded.
    Settle(u64, bool),
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        30 => (any::<usize>(), answer()).prop_map(|(pick, answer)| Step::Answer(pick, Box::new(answer))),
        8 => transaction().prop_map(|tx| Step::Transaction(Box::new(tx))),
        3 => (validated_ledger(), fee(), reserve(), reserve())
            .prop_map(|((a, t), b, c, d)| Step::LedgerClosed([a, t, b, c, d])),
        1 => prop::collection::vec(any::<u8>(), 0..64).prop_map(Step::Raw),
        1 => Just(Step::Connect),
        1 => any::<bool>().prop_map(Step::Disconnect),
        1 => any::<bool>().prop_map(Step::Funded),
        1 => (0u64..40_000).prop_map(Step::Timer),
        2 => (prop_oneof![1u64..1_000_000, 1u64..100_000_000_000], any::<bool>())
            .prop_map(|(drops, execute)| Step::Settle(drops, execute)),
    ]
}

/// A state as the scheme writes it, with any values, and up to
/// `in_flight_count` settlements on their way.
fn state_value(in_flight_count: usize) -> impl Strategy<Value = Value> {
    let blob = field(prop::collection::vec(any::<u8>(), 0..80).prop_map(Leaf::Hex));
    let executed = field(prop_oneof![
        1 => Just(Leaf::Absent),
        2 => any::<bool>().prop_map(Leaf::Bool),
    ]);
    let in_flight = (
        blob,
        drops_number(),
        drops_number(),
        index(),
        index(),
        index(),
        executed,
    )
        .prop_map(
            |(blob, drops, fee, sequence, last_ledger, submitted_at, executed)| {
                object(
                    NONE,
                    &[
                        ("blob", &blob),
                        ("drops", &drops),
                        ("fee", &fee),
                        ("sequence", &sequence),
                        ("last_ledger", &last_ledger),
                        ("submitted_at", &submitted_at),
                        ("executed", &executed),
                    ],
                )
            },
        );
    let name = prop_oneof![
        3 => Just("peer".to_string()),
        1 => "[a-z0-9]{1,12}",
        1 => "\\PC{0,8}",
    ];
    let peer = (name, account(1, 3));
    let recovering = prop_oneof![
        4 => any::<[u8; 32]>().prop_map(|hash| Leaf::Hex(hash.to_vec())),
        1 => junk(),
    ];
    let cursor = prop_oneof![
        1 => Just(Value::Null),
        3 => (index(), index()).prop_map(|(ledger, at)| {
            Value::from(vec![ledger.value(NONE).unwrap_or_default(), at.value(NONE).unwrap_or_default()])
        }),
        1 => junk().prop_map(|leaf| leaf.value(NONE).unwrap_or_default()),
    ];
    (
        cursor,
        prop_oneof![4 => (0u64..64).prop_map(Leaf::Index), 1 => index()],
        prop::collection::vec(in_flight, 0..=in_flight_count),
        prop::collection::vec(peer, 0..4),
        prop_oneof![
            1 => Just(None),
            4 => prop::collection::vec(recovering, 0..4).prop_map(Some),
        ],
    )
        .prop_map(|(cursor, sequence, in_flight, peers, recovering)| {
            let peers: Map<String, Value> = peers
                .into_iter()
                .filter_map(|(name, address)| Some((name, address.value(NONE)?)))
                .collect();
            let mut state = with(
                object(NONE, &[("sequence", &sequence)]),
                vec![
                    ("cursor", cursor),
                    ("in_flight", Value::from(in_flight)),
                    ("peers", Value::Object(peers)),
                ],
            );
            if let Some(recovering) = recovering {
                let recovering: Vec<Value> = recovering
                    .iter()
                    .filter_map(|leaf| leaf.value(NONE))
                    .collect();
                state = with(state, vec![("recovering", Value::from(recovering))]);
            }
            state
        })
}

/// A state the scheme could have written: settlements on their way, the
/// peer's address, and where it had read to.
fn kept_state() -> impl Strategy<Value = Vec<u8>> {
    let in_flight = (
        prop::collection::vec(any::<u8>(), 1..80),
        (1u64..10_000_000, 10u64..1_000),
        (0u32..64, 0u32..64),
        prop::option::of(any::<bool>()),
    );
    let cursor = prop::option::of((0u32..32, prop_oneof![Just(u32::MAX), 0u32..8]));
    (
        cursor,
        1u32..64,
        prop::collection::vec(in_flight, 0..4),
        prop::collection::vec(any::<[u8; 32]>(), 0..2),
    )
        .prop_map(|(cursor, first, in_flight, recovering)| {
            let in_flight: Vec<Value> = in_flight
                .iter()
                .zip(first..)
                .map(|((blob, (drops, fee), (last_ledger, submitted_at), executed), sequence)| {
                    let entry = json!({
                        "blob": hex_upper(blob), "drops": drops, "fee": fee, "sequence": sequence,
                        "last_ledger": last_ledger, "submitted_at": submitted_at,
                    });
                    match executed {
                        Some(executed) => with(entry, vec![("executed", Value::from(*executed))]),
                        None => entry,
                    }
                })
                .collect();
            let next = first + u32::try_from(in_flight.len()).unwrap();
            let recovering: Vec<String> = recovering.iter().map(|hash| hex_upper(hash)).collect();
            json!({
                "cursor": cursor.map(|(ledger, at)| [ledger, at]),
                "sequence": next,
                "in_flight": in_flight,
                "peers": { "peer": theirs().to_address() },
                "recovering": recovering,
            })
            .to_string()
            .into_bytes()
        })
}

/// Drops as a JSON number, as the state keeps them.
fn drops_number() -> BoxedStrategy<Leaf> {
    field(prop_oneof![4 => 0u64..10_000_000, 1 => any::<u64>()].prop_map(Leaf::Index))
}

/// The scheme, driven as the node drives it, against a ledger that says
/// anything.
struct Run {
    scheme: XrplScheme,
    now: u64,
    /// Requests sent and not yet answered: ID, and the hash it names.
    sent: Vec<(u64, Option<[u8; 32]>)>,
    /// Hashes the scheme knows: its settlements, and those it looks for.
    hashes: Vec<[u8; 32]>,
    /// The network's ledger: one more each step.
    ledger: u64,
    rng: SeededRng,
}

impl Run {
    fn new(scheme: XrplScheme) -> Self {
        let state: Value = serde_json::from_slice(&scheme.state()).unwrap();
        let blobs = state["in_flight"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| codec::hash(&from_hex(t["blob"].as_str().unwrap()).unwrap()));
        let recovering = state["recovering"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| from_hex(h.as_str().unwrap()).unwrap().try_into().unwrap());
        let hashes = blobs.chain(recovering).collect();
        Self {
            scheme,
            now: 1,
            sent: Vec::new(),
            hashes,
            ledger: 16,
            rng: SeededRng::new(1),
        }
    }

    fn input(&mut self, input: LedgerInput<'_>) {
        let mut effects = Vec::new();
        self.scheme.on_ledger(self.now, input, &mut effects);
        self.apply(effects);
    }

    fn message(&mut self, message: &Value) {
        self.input(LedgerInput::Message(&serde_json::to_vec(message).unwrap()));
    }

    /// Carries out what the scheme asks, as the node would.
    fn apply(&mut self, effects: Vec<Effect>) {
        self.scheme.check_invariants();
        let _ = (
            self.scheme.state(),
            self.scheme.balance(),
            self.scheme.status(),
        );
        for effect in effects {
            match effect {
                Effect::LedgerSend { message } => {
                    let request: Value = serde_json::from_slice(&message).unwrap();
                    let hash = request["transaction"]
                        .as_str()
                        .and_then(from_hex)
                        .and_then(|hash| hash.try_into().ok());
                    self.sent.push((request["id"].as_u64().unwrap(), hash));
                }
                Effect::Reconnect => {
                    self.sent.clear();
                    self.input(LedgerInput::Connected);
                }
                _ => {}
            }
        }
    }

    fn step(&mut self, step: &Step) {
        // A scheme that stopped is added again by its owner, from the
        // state the books kept.
        if matches!(self.scheme.status(), LedgerStatus::Failed(_)) {
            let state = self.scheme.state();
            let balance = self.scheme.balance();
            self.scheme = XrplScheme::new("", &ENTROPY, node(), Some(balance), &state).unwrap();
            self.sent.clear();
            self.input(LedgerInput::Connected);
        }
        self.ledger += 1;
        let ledger = self.ledger;
        match step {
            Step::Answer(pick, answer) => {
                let (id, hash) = if self.sent.is_empty() {
                    (u64::try_from(*pick).unwrap_or(u64::MAX), None)
                } else {
                    self.sent.remove(pick % self.sent.len())
                };
                // An answer to a lookup names the hash looked for.
                let hashes = hash.map_or_else(|| self.hashes.clone(), |hash| vec![hash]);
                let context = Context {
                    hashes: &hashes,
                    ledger,
                };
                self.message(&answer.message(id, context));
            }
            Step::Transaction(tx) => {
                let message = tx.message(Context {
                    hashes: &self.hashes,
                    ledger,
                });
                self.message(&message);
            }
            Step::LedgerClosed(
                [
                    ledger_index,
                    ledger_time,
                    fee_base,
                    reserve_base,
                    reserve_inc,
                ],
            ) => {
                let message = with(
                    object(
                        Context {
                            hashes: &[],
                            ledger,
                        },
                        &[
                            ("ledger_index", ledger_index),
                            ("ledger_time", ledger_time),
                            ("fee_base", fee_base),
                            ("reserve_base", reserve_base),
                            ("reserve_inc", reserve_inc),
                        ],
                    ),
                    vec![("type", "ledgerClosed".into())],
                );
                self.message(&message);
            }
            Step::Raw(bytes) => self.input(LedgerInput::Message(bytes)),
            Step::Connect => {
                self.sent.clear();
                self.input(LedgerInput::Connected);
            }
            Step::Disconnect(again) => {
                self.sent.clear();
                self.input(LedgerInput::Disconnected);
                if *again {
                    self.input(LedgerInput::Connected);
                }
            }
            Step::Funded(funded) => self.input(LedgerInput::Funded(*funded)),
            Step::Timer(after) => {
                self.now += after;
                let mut effects = Vec::new();
                self.scheme.on_timer(self.now, &mut effects);
                self.apply(effects);
            }
            Step::Settle(drops, execute) => {
                let prepared = self.scheme.prepare_settlement(
                    &mut self.rng,
                    peer(),
                    drops * UNITS_PER_DROP,
                    &address_bytes(theirs()),
                );
                let mut effects = Vec::new();
                if let Ok(prepared) = prepared {
                    self.hashes.push(prepared.message.try_into().unwrap());
                    if *execute {
                        self.scheme.execute_settlement(prepared.id, &mut effects);
                    } else {
                        self.scheme.discard_settlement(prepared.id);
                    }
                }
                self.apply(effects);
            }
        }
    }
}

/// A scheme up to date with a ledger on which its account holds 1,000 XRP.
fn synced_scheme() -> XrplScheme {
    let mut run = Run::new(scheme());
    run.input(LedgerInput::Connected);
    let results = [
        json!({ "state": { "network_id": 1, "validated_ledger":
            { "seq": 10, "close_time": 10, "base_fee": 10, "reserve_base": 1_000_000,
              "reserve_inc": 200_000 } } }),
        json!({}),
        json!({ "account_data": { "Balance": "1000000000", "Sequence": 5, "OwnerCount": 0 },
            "ledger_index": 10 }),
    ];
    for result in results {
        let (id, _) = run.sent.remove(0);
        run.message(
            &json!({ "type": "response", "id": id, "status": "success", "result": result }),
        );
    }
    assert_eq!(run.scheme.status(), LedgerStatus::Synced);
    run.scheme
}

/// Peering info, a request or a settlement claim from a peer: anything,
/// and valid ones changed.
fn peer_bytes() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        prop::collection::vec(any::<u8>(), 0..64),
        (any::<u8>(), "\\PC{0,40}").prop_map(|(length, text)| {
            let mut bytes = vec![length];
            bytes.extend_from_slice(text.as_bytes());
            bytes
        }),
        (any::<[u8; 20]>(), mutations())
            .prop_map(|(id, mutations)| mutate(address_bytes(AccountId(id)), &mutations)),
        mutations().prop_map(|mutations| mutate(address_bytes(theirs()), &mutations)),
    ]
}

proptest! {
    #[test]
    fn decoders_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..400), text in "\\PC{0,60}") {
        let _ = Payment::decode(&bytes);
        let _ = TrustSet::decode(&bytes);
        let _ = Transaction::decode(&bytes);
        let _ = api::parse(&bytes);
        let _ = TokenValue::parse(&text);
        let _ = Currency::parse(&text);
        let _ = base58::decode_check(&text, &[0]);
        let _ = AccountId::parse(&text);
    }

    #[test]
    fn payments_decode_what_was_changed_or_refuse_it(payment in payment(), mutations in mutations()) {
        let blob = payment.blob();
        prop_assert_eq!(Payment::decode(&blob), Ok(payment));
        // What decodes from changed bytes is a Payment the codec can
        // write again.
        if let Ok(decoded) = Payment::decode(&mutate(blob, &mutations)) {
            prop_assert_eq!(Payment::decode(&decoded.blob()), Ok(decoded));
        }
    }

    #[test]
    fn trust_sets_decode_what_was_changed_or_refuse_it(trust_set in trust_set(), mutations in mutations()) {
        let blob = trust_set.blob();
        prop_assert_eq!(TrustSet::decode(&blob), Ok(trust_set.clone()));
        prop_assert_eq!(Transaction::decode(&blob), Ok(Transaction::TrustSet(trust_set)));
        if let Ok(decoded) = Transaction::decode(&mutate(blob, &mutations)) {
            prop_assert_eq!(Transaction::decode(&decoded.blob()), Ok(decoded));
        }
    }

    // A value has one form, in bytes and in text, and its order is its
    // worth (spec/areas/xrpl.md#tokens).
    #[test]
    fn token_values_have_one_form(value in token_value(), other in token_value()) {
        prop_assert_eq!(TokenValue::from_bits(value.to_bits()), Some(value));
        prop_assert_eq!(TokenValue::parse(&value.to_string()), Some(value));
        prop_assert_eq!(
            TokenValue::parse(&format!("{}{}e{}", if value.is_negative() { "-" } else { "" }, value.mantissa(), value.exponent())),
            Some(value)
        );
        prop_assert_eq!(value.negate().negate(), value);
        prop_assert_eq!(value < other, other.negate() < value.negate());
        prop_assert_eq!(value == other, value.to_bits() == other.to_bits());
        let bytes = TokenAmount { value, currency: Currency::parse(RLUSD).unwrap(), issuer: theirs() }.to_bytes();
        prop_assert_eq!(TokenAmount::from_bytes(&bytes).map(|amount| amount.value), Some(value));
    }

    // Whole units round down, toward minus infinity, exactly.
    #[test]
    fn whole_units_round_down(value in token_value(), decimals in 0..=40u8) {
        let Some(units) = value.floor_units(decimals) else {
            // Only what is beyond an i128 is refused.
            prop_assert!(value.exponent() + i32::from(decimals) > 21);
            return Ok(());
        };
        let signed = if value.is_negative() { -i128::from(value.mantissa()) } else { i128::from(value.mantissa()) };
        let shift = value.exponent() + i32::from(decimals);
        if value.is_zero() {
            prop_assert_eq!(units, 0);
        } else if shift >= 0 {
            prop_assert_eq!(Some(units), signed.checked_mul(10_i128.pow(shift.unsigned_abs())));
        } else if shift < -30 {
            prop_assert_eq!(units, if value.is_negative() { -1 } else { 0 });
        } else {
            let unit = 10_i128.pow(shift.unsigned_abs());
            prop_assert!(units * unit <= signed && signed < (units + 1) * unit);
        }
    }

    // Whole units become a value exactly or not at all, and back.
    #[test]
    fn units_convert_exactly(units in prop_oneof![any::<u64>(), 0..10_000_000_000_000_000u64], decimals in 0..=MAX_DECIMALS) {
        let value = TokenValue::from_units(units, decimals);
        if let Some(value) = value {
            prop_assert!(!value.is_negative());
            prop_assert_eq!(value.floor_units(decimals), Some(i128::from(units)));
        } else {
            // More than sixteen digits, or below the least value.
            let digits = units.to_string();
            let significant = digits.trim_end_matches('0').len();
            let exponent = i32::try_from(digits.len()).unwrap() - 16 - i32::from(decimals);
            prop_assert!(significant > 16 || exponent < MIN_EXPONENT);
        }
    }

    // A trust line reads the same from either side: one side's balance is
    // the other's negated, and each side's limit and flags are its own,
    // whichever of the two is the lower number.
    #[test]
    fn trust_lines_read_the_same_from_either_side(
        (a, b) in two_accounts(),
        currency in currency(),
        balance in token_value(),
        (limit_a, limit_b) in (token_value(), token_value()),
        (bits_a, bits_b) in (0..32u32, 0..32u32),
    ) {
        let state = ripple_state((a, b), currency, balance, (limit_a, limit_b), (side_flags(bits_a), side_flags(bits_b)));
        let result = json!({ "node": state, "ledger_index": 9, "validated": true });
        let entry = api::ripple_state_result(&result).unwrap();
        prop_assert_eq!(entry.ledger_index, 9);
        prop_assert!(entry.validated);
        let line = entry.line;
        prop_assert!(line.low < line.high);
        prop_assert!(line.is_between(a, b, currency) && line.is_between(b, a, currency));
        let (from_a, from_b) = (line.view(a).unwrap(), line.view(b).unwrap());
        prop_assert_eq!(from_a.counterparty, b);
        prop_assert_eq!(from_b.counterparty, a);
        prop_assert_eq!(from_a.balance, balance);
        prop_assert_eq!(from_b.balance, balance.negate());
        prop_assert_eq!((from_a.limit, from_a.counterparty_limit), (limit_a, limit_b));
        prop_assert_eq!((from_b.limit, from_b.counterparty_limit), (limit_b, limit_a));
        prop_assert_eq!(from_a.ours, expected_flags(bits_a));
        prop_assert_eq!(from_a.theirs, expected_flags(bits_b));
        prop_assert_eq!(from_b.ours, from_a.theirs);
        prop_assert_eq!(from_b.theirs, from_a.ours);
        prop_assert!(line.view(ours()).is_none() || ours() == a || ours() == b);
        // Limits the wrong way round are no line.
        let mut swapped = result;
        let (low, high) = (swapped["node"]["LowLimit"].clone(), swapped["node"]["HighLimit"].clone());
        swapped["node"]["LowLimit"] = high;
        swapped["node"]["HighLimit"] = low;
        prop_assert!(api::ripple_state_result(&swapped).is_none());
    }

    // What a transaction did to a line, from either side: created (from
    // zero), changed, or deleted (to zero).
    #[test]
    fn line_changes_read_the_same_from_either_side(
        (a, b) in two_accounts(),
        currency in currency(),
        (before, after) in (token_value(), token_value()),
        kind in 0..3u8,
        delivered in token_amount(),
    ) {
        let state = |balance| ripple_state((a, b), currency, balance, (TokenValue::MAX, TokenValue::ZERO), (side_flags(4), 0));
        let previous = state(before)["Balance"].clone();
        let (node, expected) = match kind {
            0 => (json!({ "CreatedNode": { "LedgerEntryType": "RippleState", "NewFields": state(after) } }), (TokenValue::ZERO, after)),
            1 => (
                json!({ "ModifiedNode": { "LedgerEntryType": "RippleState", "FinalFields": state(after), "PreviousFields": { "Balance": previous } } }),
                (before, after),
            ),
            _ => (
                json!({ "DeletedNode": { "LedgerEntryType": "RippleState", "FinalFields": state(after), "PreviousFields": { "Balance": previous } } }),
                (before, TokenValue::ZERO),
            ),
        };
        let delivered_json = amount_json(delivered.value, delivered.currency, &delivered.issuer.to_address());
        let record = api::tx_result(&tx_with(a, vec![node], Some(delivered_json))).unwrap();
        prop_assert_eq!(record.delivered_token, Some(delivered));
        prop_assert_eq!(record.delivered, None);
        let change = record.line(a, b, currency).unwrap();
        prop_assert_eq!(record.line(b, a, currency), Some(change));
        prop_assert_eq!(change.balance_of(a), Some(expected));
        prop_assert_eq!(change.balance_of(b), Some((expected.0.negate(), expected.1.negate())));
        prop_assert_eq!(change.line().view(a).unwrap().ours.no_ripple, true);
        let third = AccountId([0xEE; 20]);
        if third != a && third != b {
            prop_assert_eq!(change.balance_of(third), None);
            prop_assert!(record.line(a, third, currency).is_none());
        }
    }

    #[test]
    fn configs_parse_or_are_refused(text in config_text()) {
        let scheme = XrplScheme::new(&text, &ENTROPY, node(), None, &[]);
        match Config::parse(&text) {
            Ok(config) => {
                prop_assert!(config.server.starts_with("wss://") || config.server.starts_with("ws://"));
                prop_assert!(config.faucet.as_ref().is_none_or(|faucet| faucet.starts_with("https://")));
                let scheme = scheme.unwrap();
                scheme.check_invariants();
                prop_assert_eq!(scheme.config(), &config);
            }
            Err(_) => prop_assert!(scheme.is_err()),
        }
    }

    #[test]
    fn states_restore_or_are_refused(
        state in prop_oneof![
            2 => kept_state(),
            2 => state_value(9).prop_map(|state| state.to_string().into_bytes()),
            1 => prop::collection::vec(any::<u8>(), 0..200),
        ],
        mutations in mutations(),
        checkpoint in prop::option::of(0..=MAX_TOTAL),
    ) {
        let state = mutate(state, &mutations);
        if let Ok(scheme) = XrplScheme::new("", &ENTROPY, node(), checkpoint, &state) {
            scheme.check_invariants();
            // What it keeps of it, it can start from again.
            let kept = scheme.state();
            let again = XrplScheme::new("", &ENTROPY, node(), checkpoint, &kept).unwrap();
            prop_assert_eq!(again.state(), kept);
        }
    }

    #[test]
    fn peers_cannot_make_the_scheme_panic(
        calls in prop::collection::vec(
            (0u8..6, prop::sample::select(&["peer", "other", "third"][..]), peer_bytes(), peer_bytes(),
             prop_oneof![1u64..1_000_000_000_000, any::<u64>()]),
            1..20,
        ),
    ) {
        let mut scheme = synced_scheme();
        let mut rng = SeededRng::new(1);
        for (call, peer, first, second, amount) in calls {
            let peer = Name::new(peer).unwrap();
            let mut out = Vec::new();
            let mut effects = Vec::new();
            match call {
                0 => {
                    let _ = scheme.create_peering_request(peer, &first, &mut out);
                }
                1 => {
                    let _ = scheme.accept_peering_request(peer, &first, &mut out);
                }
                2 => {
                    let _ = scheme.finalize_peering_request(peer, &first, &second);
                }
                3 => {
                    if let Ok(prepared) = scheme.prepare_settlement(&mut rng, peer, amount, &first) {
                        scheme.execute_settlement(prepared.id, &mut effects);
                    }
                }
                4 => {
                    let _ = scheme.handle_settlement(peer, amount, &first, &second, &mut effects);
                }
                _ => scheme.handle_message(peer, &first, &mut effects),
            }
            scheme.check_invariants();
        }
        let state = scheme.state();
        prop_assert!(XrplScheme::new("", &ENTROPY, node(), Some(scheme.balance()), &state).is_ok());
    }

    #[test]
    fn the_scheme_takes_anything_the_ledger_says(
        state in prop_oneof![
            1 => Just(Vec::new()),
            3 => kept_state(),
            1 => state_value(2).prop_map(|state| state.to_string().into_bytes()),
        ],
        checkpoint in prop::option::of(0..=MAX_TOTAL),
        recovering in prop::collection::vec(any::<[u8; 32]>(), 0..3),
        steps in prop::collection::vec(step(), 1..120),
    ) {
        let scheme = XrplScheme::new("", &ENTROPY, node(), checkpoint, &state)
            .or_else(|_| XrplScheme::new("", &ENTROPY, node(), checkpoint, &[]))
            .unwrap();
        let mut run = Run::new(scheme);
        // The peer, as peering told it.
        run.scheme.on_peered(peer(), &address_bytes(theirs()));
        // Settlements a restart left pending, to look for.
        let pending: Vec<(Name, u64)> =
            recovering.iter().map(|hash| (Name::hex(hash, true), 1)).collect();
        let mut effects = Vec::new();
        SettlementScheme::reconcile(&mut run.scheme, &pending, &mut effects);
        run.hashes.extend(recovering);
        run.apply(effects);
        run.input(LedgerInput::Connected);
        for step in &steps {
            run.step(step);
        }
        let mut effects = Vec::new();
        run.scheme.on_timer(u64::MAX / 2, &mut effects);
        run.apply(effects);
        // The state it keeps is one it can start from again.
        let state = run.scheme.state();
        prop_assert!(XrplScheme::new("", &ENTROPY, node(), Some(run.scheme.balance()), &state).is_ok());
    }
}
