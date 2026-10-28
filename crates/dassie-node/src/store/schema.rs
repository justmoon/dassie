//! The schema, its migrations, and the check that the file matches it
//! (spec/areas/node.md#database).
//!
//! An application ID marks the file as Dassie's, `user_version` counts the
//! migrations run, each in a transaction, every table is STRICT, and a check
//! at every open finds each table as the code expects it, an extra column
//! being an error too. The first migration is the whole schema, and the
//! early testnet's databases are refused
//! (spec/decisions/database-continuity.md).

use rusqlite::{Connection, OptionalExtension, Transaction};

use super::StoreError;

/// Marks a file as a Dassie database: a random positive int32 with nibbles
/// 2–4 set to `DA5`.
pub const APPLICATION_ID: i32 = 0x5DA5_FE32;

/// The ID of the early testnet's databases, whose schema this one does not
/// continue: they are refused (spec/decisions/database-continuity.md).
pub const EARLY_TESTNET_APPLICATION_ID: i32 = 0x7DA5_E2C9;

/// The migrations, in order. Migration `n` (1-based) brings `user_version`
/// to `n`. Never edit one that has shipped; add another.
pub(super) const MIGRATIONS: &[&str] = &[
    BASELINE,
    MIGRATION_2,
    MIGRATION_3,
    MIGRATION_4,
    MIGRATION_5,
    MIGRATION_6,
    MIGRATION_7,
    MIGRATION_8,
    MIGRATION_9,
    MIGRATION_10,
    MIGRATION_11,
];

/// The schema version this code writes: the number of migrations.
pub const SCHEMA_VERSION: i64 = version_of(MIGRATIONS);

/// The schema version `migrations` bring a database to.
// A handful of migrations: the length never comes near `i64::MAX`.
#[allow(clippy::cast_possible_wrap)]
const fn version_of(migrations: &[&str]) -> i64 {
    migrations.len() as i64
}

/// Migration 1: the whole schema, with the rows a new node starts with.
/// Each table's purpose is in the area doc that the comment before it
/// names (spec/areas/node.md#persistence for what must survive a restart).
/// Amounts are integers in their ledger's units, and times milliseconds
/// since the epoch.
const BASELINE: &str = "
-- The node (spec/areas/node.md#configuration): its configuration, key,
-- TLS certificate and ACME accounts, the owner's sessions, paired devices
-- (spec/areas/rpc.md#paired-devices), whose key is public, and the setup
-- token, kept until setup completes so a restart keeps the setup URL valid.
CREATE TABLE node_config (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  realm TEXT NOT NULL CHECK (realm IN ('test', 'live')),
  hostname TEXT CHECK (hostname IS NULL OR (length(hostname) BETWEEN 1 AND 253 AND hostname = lower(hostname))),
  alias TEXT NOT NULL CHECK (length(CAST(alias AS BLOB)) <= 64),
  acme_directory_url TEXT NOT NULL CHECK (acme_directory_url LIKE 'https://%')
) STRICT;

INSERT INTO node_config VALUES (1, 'test', NULL, '', 'https://acme-v02.api.letsencrypt.org/directory');

CREATE TABLE node_key (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  private_key BLOB NOT NULL CHECK (length(private_key) = 32),
  created_at_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE tls_certificate (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  source TEXT NOT NULL CHECK (source IN ('self-signed', 'acme', 'manual')),
  chain_pem TEXT NOT NULL,
  key_pem TEXT NOT NULL,
  installed_at_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE acme_account (
  directory_url TEXT PRIMARY KEY,
  credentials TEXT NOT NULL,
  contact TEXT NOT NULL,
  created_at_ms INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

CREATE TABLE sessions (
  id BLOB PRIMARY KEY CHECK (length(id) = 32),
  created_at_ms INTEGER NOT NULL,
  last_used_at_ms INTEGER NOT NULL CHECK (last_used_at_ms >= created_at_ms),
  user_agent TEXT NOT NULL
) STRICT, WITHOUT ROWID;

CREATE INDEX sessions_by_last_use ON sessions (last_used_at_ms);

CREATE TABLE devices (
  id TEXT PRIMARY KEY CHECK (length(id) BETWEEN 2 AND 45),
  name TEXT NOT NULL CHECK (length(CAST(name AS BLOB)) BETWEEN 1 AND 64),
  paired_at_ms INTEGER NOT NULL,
  last_seen_at_ms INTEGER NOT NULL CHECK (last_seen_at_ms >= paired_at_ms)
) STRICT, WITHOUT ROWID;

CREATE TABLE setup_token (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  token BLOB NOT NULL CHECK (length(token) = 32)
) STRICT;

-- The ledger (spec/areas/ledger.md): the boot epoch that packets are traced
-- by, the accounts with their posted totals cached, which the ledger keeps
-- below i64::MAX, and the journal of transfers.
CREATE TABLE boot (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  epoch INTEGER NOT NULL CHECK (epoch >= 0)
) STRICT;

INSERT INTO boot VALUES (1, 0);

CREATE TABLE ledger_accounts (
  id INTEGER PRIMARY KEY CHECK (id >= 0),
  path TEXT NOT NULL UNIQUE,
  debits_posted INTEGER NOT NULL DEFAULT 0 CHECK (debits_posted >= 0),
  credits_posted INTEGER NOT NULL DEFAULT 0 CHECK (credits_posted >= 0)
) STRICT;

CREATE TABLE ledger_transfers (
  id INTEGER PRIMARY KEY CHECK (id >= 0),
  debit INTEGER NOT NULL REFERENCES ledger_accounts (id),
  credit INTEGER NOT NULL REFERENCES ledger_accounts (id) CHECK (credit != debit),
  amount INTEGER NOT NULL CHECK (amount > 0),
  state TEXT NOT NULL CHECK (state IN ('pending', 'posted', 'voided')),
  created_at_ms INTEGER NOT NULL,
  resolved_at_ms INTEGER CHECK ((state = 'pending') = (resolved_at_ms IS NULL)),
  reference TEXT NOT NULL CHECK (reference IN
    ('packet', 'settlement', 'trust', 'deposit', 'withdrawal', 'reconciliation', 'fee', 'loss',
     'owner_payment')),
  packet_epoch INTEGER CHECK ((reference = 'packet') = (packet_epoch IS NOT NULL)),
  packet_sequence INTEGER CHECK ((reference = 'packet') = (packet_sequence IS NOT NULL)),
  settlement_scheme TEXT
    CHECK ((reference IN ('settlement', 'fee', 'loss', 'owner_payment'))
      = (settlement_scheme IS NOT NULL)),
  settlement_id TEXT
    CHECK ((reference IN ('settlement', 'fee', 'loss', 'owner_payment'))
      = (settlement_id IS NOT NULL)),
  -- The order rows were written or last changed in, for readers that page
  -- through the journal: a packet's transfer is written when it is posted,
  -- which is not the order of transfer IDs.
  sequence INTEGER NOT NULL UNIQUE CHECK (sequence > 0)
) STRICT;

CREATE INDEX ledger_transfers_by_debit ON ledger_transfers (debit, state);
CREATE INDEX ledger_transfers_by_credit ON ledger_transfers (credit, state);
CREATE INDEX ledger_transfers_pending ON ledger_transfers (state) WHERE state = 'pending';
CREATE INDEX ledger_transfers_by_debit_sequence ON ledger_transfers (debit, sequence);
CREATE INDEX ledger_transfers_by_credit_sequence ON ledger_transfers (credit, sequence);
-- A fee's packet is found by its reference, and where to start reading fees
-- by the last trust transfer: for credit that grew from fees, which
-- MIGRATION_4 drops with these two indexes.
CREATE INDEX ledger_transfers_by_packet ON ledger_transfers (packet_epoch, packet_sequence)
  WHERE packet_epoch IS NOT NULL;
CREATE INDEX ledger_transfers_trust ON ledger_transfers (sequence) WHERE reference = 'trust';

-- Settlement schemes and external assets (spec/areas/settlement.md): each
-- scheme's config and checkpoint, written in the batch it explains, and
-- each external asset's state and the owner's config of it
-- (spec/areas/settlement.md#external-assets).
CREATE TABLE settlement_schemes (
  id TEXT PRIMARY KEY,
  config TEXT NOT NULL,
  activated_at_ms INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

CREATE TABLE settlement_checkpoints (
  scheme TEXT PRIMARY KEY,
  balance INTEGER NOT NULL CHECK (balance >= 0),
  updated_at_ms INTEGER NOT NULL,
  state BLOB CHECK (state IS NULL OR length(state) <= 262144)
) STRICT, WITHOUT ROWID;

CREATE TABLE asset_checkpoints (
  asset TEXT PRIMARY KEY CHECK (length(asset) BETWEEN 1 AND 64),
  updated_at_ms INTEGER NOT NULL,
  state BLOB NOT NULL CHECK (length(state) <= 262144)
) STRICT, WITHOUT ROWID;

CREATE TABLE asset_configs (
  asset TEXT PRIMARY KEY CHECK (length(asset) BETWEEN 1 AND 64),
  config TEXT NOT NULL CHECK (length(CAST(config AS BLOB)) BETWEEN 1 AND 4096),
  updated_at_ms INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

-- BTP tokens, by hash (spec/decisions/secrets-at-rest.md), with their
-- spending caps (spec/areas/connector.md#spending-caps).
CREATE TABLE btp_tokens (
  id INTEGER PRIMARY KEY CHECK (id >= 1),
  token_hash BLOB NOT NULL UNIQUE CHECK (length(token_hash) = 32),
  label TEXT NOT NULL CHECK (length(CAST(label AS BLOB)) <= 64),
  ledger TEXT NOT NULL,
  created_at_ms INTEGER NOT NULL,
  revoked_at_ms INTEGER,
  -- At most cap_amount per cap_window_ms, refilling continuously.
  cap_amount INTEGER CHECK (cap_amount IS NULL OR cap_amount > 0),
  cap_window_ms INTEGER CHECK (cap_window_ms IS NULL OR cap_window_ms BETWEEN 1000 AND 31622400000),
  -- What fulfilled packets use of the cap as of cap_as_of_ms, in amount·ms:
  -- cap_used_amount · cap_window_ms + cap_used_remainder. Written with the
  -- journal rows that change it.
  cap_used_amount INTEGER NOT NULL DEFAULT 0 CHECK (cap_used_amount >= 0),
  cap_used_remainder INTEGER NOT NULL DEFAULT 0 CHECK (cap_used_remainder >= 0),
  cap_as_of_ms INTEGER NOT NULL DEFAULT 0,
  CHECK ((cap_amount IS NULL) = (cap_window_ms IS NULL)),
  CHECK (cap_window_ms IS NULL OR cap_used_remainder < cap_window_ms)
) STRICT;

-- Fees, spread and rebalancing (spec/decisions/exchange-rates.md,
-- spec/decisions/passive-rebalancing.md), the sources of exchange rates,
-- and the last rates fetched, each with when it was.
CREATE TABLE peer_settings (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  connector_fee_ppm INTEGER NOT NULL DEFAULT 0 CHECK (connector_fee_ppm BETWEEN 0 AND 100000),
  fx_spread_ppm INTEGER NOT NULL DEFAULT 0 CHECK (fx_spread_ppm BETWEEN 0 AND 100000),
  rebalance_ppm INTEGER NOT NULL DEFAULT 50000 CHECK (rebalance_ppm BETWEEN 0 AND 100000)
) STRICT;

INSERT INTO peer_settings VALUES (1, 0, 0, 50000);

CREATE TABLE rate_sources (
  position INTEGER PRIMARY KEY CHECK (position BETWEEN 0 AND 7),
  kind TEXT NOT NULL CHECK (kind IN ('coinbase', 'kraken', 'bitstamp', 'ecb')),
  -- Only the Coinbase format is served by others too.
  url TEXT CHECK (url IS NULL OR (kind = 'coinbase' AND length(url) BETWEEN 9 AND 2048))
) STRICT;

INSERT INTO rate_sources VALUES
  (0, 'coinbase', NULL), (1, 'kraken', NULL), (2, 'bitstamp', NULL), (3, 'ecb', NULL);

CREATE TABLE exchange_rates (
  asset TEXT PRIMARY KEY CHECK (length(asset) BETWEEN 1 AND 8),
  -- Units of the asset one US dollar buys, times 10^15, in decimal: it may
  -- not fit 64 bits.
  rate TEXT NOT NULL CHECK (length(rate) BETWEEN 1 AND 39
    AND rate NOT GLOB '*[^0-9]*' AND rate NOT GLOB '0*'),
  as_of_ms INTEGER NOT NULL CHECK (as_of_ms >= 0)
) STRICT, WITHOUT ROWID;

-- The network (spec/areas/network.md#persistence): the node table, with
-- each node's verified link state and its registration with us; our own
-- link state's sequence, which only goes up; and bans.
CREATE TABLE peer_nodes (
  id TEXT PRIMARY KEY CHECK (length(id) BETWEEN 2 AND 45),
  link_state BLOB CHECK (link_state IS NULL OR length(link_state) <= 8192),
  sources INTEGER NOT NULL CHECK (sources BETWEEN 0 AND 31),
  first_seen_ms INTEGER NOT NULL,
  registered_at_ms INTEGER,
  renewed_at_ms INTEGER,
  registration_expires_ms INTEGER,
  -- The URL the registration was probed at, and the /24 or /48 it came
  -- from, which registrations are capped by.
  registered_url TEXT,
  registered_network BLOB CHECK (registered_network IS NULL OR length(registered_network) = 7),
  CHECK ((registered_at_ms IS NULL) = (renewed_at_ms IS NULL)
     AND (renewed_at_ms IS NULL) = (registration_expires_ms IS NULL)
     AND (registration_expires_ms IS NULL) = (registered_url IS NULL))
) STRICT, WITHOUT ROWID;

CREATE TABLE own_link_state (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  sequence INTEGER NOT NULL CHECK (sequence >= 0)
) STRICT;

INSERT INTO own_link_state VALUES (1, 0);

CREATE TABLE peer_bans (
  node TEXT PRIMARY KEY CHECK (length(node) BETWEEN 2 AND 45)
) STRICT, WITHOUT ROWID;

-- Peerings and settlement (spec/areas/settlement.md): our peering with a
-- node, a peering being ended, the settlement channels, which outlive
-- peerings, and the peers' accounts opened for downlinks
-- (spec/decisions/account-budgets.md).
CREATE TABLE peers (
  node TEXT PRIMARY KEY REFERENCES peer_nodes (id) ON DELETE CASCADE,
  scheme TEXT NOT NULL,
  state BLOB NOT NULL,
  credit_out INTEGER NOT NULL CHECK (credit_out >= 0),
  since_ms INTEGER NOT NULL,
  inbound INTEGER NOT NULL CHECK (inbound IN (0, 1)),
  last_seen_ms INTEGER NOT NULL,
  -- How long the peer's room has been short of a packet, settling nothing,
  -- and how much of that it could be reached
  -- (spec/decisions/peering-lifecycle.md).
  short_for_ms INTEGER NOT NULL DEFAULT 0,
  -- A peering we are ending, while we collect first and have not told the
  -- peer (spec/decisions/ending-a-peering.md).
  depeering_since_ms INTEGER,
  depeering_reason INTEGER CHECK (depeering_reason IS NULL OR depeering_reason BETWEEN 0 AND 255),
  short_reached_ms INTEGER NOT NULL DEFAULT 0
) STRICT, WITHOUT ROWID;

CREATE TABLE peer_terminations (
  node TEXT PRIMARY KEY REFERENCES peers (node) ON DELETE CASCADE,
  since_ms INTEGER NOT NULL,
  reason INTEGER NOT NULL CHECK (reason BETWEEN 0 AND 255),
  by_us INTEGER NOT NULL CHECK (by_us IN (0, 1)),
  acknowledged INTEGER NOT NULL CHECK (acknowledged IN (0, 1))
) STRICT, WITHOUT ROWID;

-- Not tied to peer_nodes: a channel stays when its node leaves the table.
CREATE TABLE settlement_channels (
  node TEXT PRIMARY KEY CHECK (length(node) BETWEEN 2 AND 45),
  public_key BLOB NOT NULL CHECK (length(public_key) = 32),
  url TEXT NOT NULL CHECK (length(url) <= 256),
  settled_in INTEGER NOT NULL CHECK (settled_in >= 0),
  settled_out INTEGER NOT NULL CHECK (settled_out >= 0),
  -- The settlement message on its way, if any: always the last one.
  outbox_sequence INTEGER CHECK (outbox_sequence IS NULL OR outbox_sequence = settled_out),
  outbox_scheme TEXT,
  outbox_amount INTEGER CHECK (outbox_amount IS NULL OR outbox_amount >= 0),
  outbox_data BLOB,
  outbox_attempts INTEGER CHECK (outbox_attempts IS NULL OR outbox_attempts >= 0),
  -- Whether we once ended a peering with the node for want of room
  -- (spec/decisions/ended-for-room.md).
  ended_for_room INTEGER NOT NULL DEFAULT 0 CHECK (ended_for_room IN (0, 1)),
  -- An idle channel closes, its row kept (spec/decisions/closing-channels.md);
  -- only one with nothing on its way.
  closed INTEGER NOT NULL DEFAULT 0
    CHECK (closed IN (0, 1) AND (closed = 0 OR outbox_sequence IS NULL)),
  -- Whether the node ever paid us a settlement of at least one largest
  -- packet (spec/decisions/free-receivers-half.md).
  paid_us INTEGER NOT NULL DEFAULT 0 CHECK (paid_us IN (0, 1)),
  CHECK ((outbox_sequence IS NULL) = (outbox_scheme IS NULL)
     AND (outbox_scheme IS NULL) = (outbox_amount IS NULL)
     AND (outbox_amount IS NULL) = (outbox_data IS NULL)
     AND (outbox_data IS NULL) = (outbox_attempts IS NULL))
) STRICT, WITHOUT ROWID;

CREATE TABLE downlink_accounts (
  ledger TEXT NOT NULL CHECK (length(ledger) BETWEEN 1 AND 32),
  node TEXT NOT NULL CHECK (length(node) BETWEEN 2 AND 45),
  PRIMARY KEY (ledger, node)
) STRICT, WITHOUT ROWID;

-- Names (spec/areas/network.md#names): a registrar's log, which only
-- rises, the commitments in it with their unsealings, and the names
-- granted, held or freed a short while ago; the node's view of who holds
-- each name (spec/areas/network.md#holders); and the node's own
-- registrations, each with the salt its commitment is sealed with
-- (spec/areas/network.md#the-registrant).
CREATE TABLE name_log (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  next_position INTEGER NOT NULL CHECK (next_position >= 0),
  last_logged_ms INTEGER NOT NULL CHECK (last_logged_ms >= 0)
) STRICT;

INSERT INTO name_log VALUES (1, 0, 0);

CREATE TABLE name_commitments (
  commitment BLOB PRIMARY KEY CHECK (length(commitment) = 32),
  position INTEGER NOT NULL UNIQUE CHECK (position >= 0),
  logged_ms INTEGER NOT NULL,
  window_end_ms INTEGER NOT NULL CHECK (window_end_ms >= logged_ms),
  committer TEXT NOT NULL CHECK (length(committer) BETWEEN 2 AND 45),
  network BLOB CHECK (length(network) = 7),
  -- Its unsealing, once unsealed.
  name TEXT CHECK (length(name) BETWEEN 3 AND 32),
  holder TEXT CHECK (length(holder) BETWEEN 2 AND 45),
  contends INTEGER CHECK (contends IN (0, 1)),
  CHECK ((name IS NULL) = (holder IS NULL) AND (holder IS NULL) = (contends IS NULL))
) STRICT, WITHOUT ROWID;

CREATE TABLE granted_names (
  name TEXT PRIMARY KEY CHECK (length(name) BETWEEN 3 AND 32),
  holder TEXT NOT NULL CHECK (length(holder) BETWEEN 2 AND 45),
  granted_ms INTEGER NOT NULL,
  expires_ms INTEGER NOT NULL CHECK (expires_ms >= granted_ms),
  freed_ms INTEGER
) STRICT, WITHOUT ROWID;

CREATE TABLE name_holders (
  name TEXT PRIMARY KEY CHECK (length(name) BETWEEN 3 AND 32),
  holder TEXT NOT NULL CHECK (length(holder) BETWEEN 2 AND 45),
  expires_ms INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

CREATE TABLE name_registrations (
  name TEXT PRIMARY KEY CHECK (length(name) BETWEEN 3 AND 32),
  salt BLOB NOT NULL CHECK (length(salt) = 32),
  started_ms INTEGER NOT NULL,
  stage TEXT NOT NULL CHECK (stage IN ('committing', 'unsealing', 'abandoned'))
) STRICT, WITHOUT ROWID;

-- Open Payments (spec/decisions/open-payments.md,
-- spec/areas/payments.md#persistence): incoming payments, written by the
-- engine with the journal rows that pay them, and what each of their STREAM
-- connections received; GNAP grants and their access tokens, by hash
-- (spec/decisions/secrets-at-rest.md); quotes; outgoing payments, written
-- with the journal rows that move their money; what each grant spent per
-- window of its interval; and which ledger `$host` names.
CREATE TABLE incoming_payments (
  id BLOB PRIMARY KEY CHECK (length(id) = 16),
  ledger TEXT NOT NULL,
  wallet_address TEXT NOT NULL CHECK (length(wallet_address) BETWEEN 1 AND 2048),
  -- The client that created it, as grants name clients; NULL for the owner.
  client TEXT CHECK (client IS NULL OR length(client) BETWEEN 1 AND 2048),
  incoming_amount INTEGER CHECK (incoming_amount IS NULL OR incoming_amount > 0),
  -- At most the tolerance over its amount, which the engine checks
  -- (spec/decisions/paid-within-a-tolerance.md).
  received_amount INTEGER NOT NULL CHECK (received_amount >= 0),
  -- STREAM credentials, the same on every read.
  ilp_address TEXT NOT NULL CHECK (length(ilp_address) BETWEEN 1 AND 1023),
  shared_secret BLOB NOT NULL CHECK (length(shared_secret) = 32),
  metadata TEXT CHECK (metadata IS NULL OR length(CAST(metadata AS BLOB)) <= 4096),
  created_at_ms INTEGER NOT NULL,
  expires_at_ms INTEGER NOT NULL CHECK (expires_at_ms > created_at_ms),
  completed_at_ms INTEGER CHECK (completed_at_ms IS NULL OR completed_at_ms >= created_at_ms),
  -- The order rows were written or last changed in, for live views.
  revision INTEGER NOT NULL UNIQUE CHECK (revision > 0)
) STRICT, WITHOUT ROWID;

CREATE INDEX incoming_payments_by_ledger ON incoming_payments (ledger, created_at_ms, id);
CREATE INDEX incoming_payments_by_client ON incoming_payments (ledger, client, created_at_ms, id);
CREATE INDEX incoming_payments_by_time ON incoming_payments (created_at_ms, id);
CREATE INDEX incoming_payments_open ON incoming_payments (expires_at_ms) WHERE completed_at_ms IS NULL;
CREATE INDEX incoming_payments_unpaid ON incoming_payments (coalesce(completed_at_ms, expires_at_ms))
  WHERE received_amount = 0;

CREATE TABLE incoming_connections (
  incoming_payment BLOB NOT NULL REFERENCES incoming_payments (id) ON DELETE CASCADE,
  -- Which credentials of the payment the connection uses.
  connection BLOB NOT NULL CHECK (length(connection) = 16),
  received INTEGER NOT NULL CHECK (received > 0),
  PRIMARY KEY (incoming_payment, connection)
) STRICT, WITHOUT ROWID;

CREATE TABLE gnap_grants (
  id BLOB PRIMARY KEY CHECK (length(id) = 16),
  state TEXT NOT NULL CHECK (state IN ('pending', 'approved', 'denied', 'revoked')),
  -- The client as resources record it: its wallet address, or its key.
  client TEXT NOT NULL CHECK (length(client) BETWEEN 1 AND 2048),
  -- The JWK every later request of the grant must be signed with.
  client_key TEXT NOT NULL CHECK (length(client_key) BETWEEN 1 AND 1024),
  access TEXT NOT NULL CHECK (length(access) BETWEEN 2 AND 16384),
  needs_owner INTEGER NOT NULL CHECK (needs_owner IN (0, 1)),
  continue_token_hash BLOB NOT NULL UNIQUE CHECK (length(continue_token_hash) = 32),
  -- The interaction, for grants the owner decides: nonces, the client's
  -- return address, the reference and the subject asked about.
  interaction TEXT CHECK (interaction IS NULL OR length(interaction) <= 4096),
  created_at_ms INTEGER NOT NULL,
  decided_at_ms INTEGER CHECK ((state = 'pending') = (decided_at_ms IS NULL)),
  -- When a pending grant lapses.
  expires_at_ms INTEGER NOT NULL CHECK (expires_at_ms > created_at_ms),
  revision INTEGER NOT NULL UNIQUE CHECK (revision > 0),
  -- The network it was asked for from, so no one network holds more than
  -- its share (spec/decisions/owners-share.md); NULL from loopback.
  network BLOB CHECK (network IS NULL OR length(network) = 16)
) STRICT, WITHOUT ROWID;

CREATE INDEX gnap_grants_by_state ON gnap_grants (state, created_at_ms);

CREATE TABLE gnap_tokens (
  token_hash BLOB PRIMARY KEY CHECK (length(token_hash) = 32),
  grant_id BLOB NOT NULL REFERENCES gnap_grants (id) ON DELETE CASCADE,
  manage_id BLOB NOT NULL UNIQUE CHECK (length(manage_id) = 16),
  created_at_ms INTEGER NOT NULL,
  expires_at_ms INTEGER NOT NULL CHECK (expires_at_ms > created_at_ms)
) STRICT, WITHOUT ROWID;

CREATE INDEX gnap_tokens_by_grant ON gnap_tokens (grant_id);
CREATE INDEX gnap_tokens_by_expiry ON gnap_tokens (expires_at_ms);

CREATE TABLE quotes (
  id BLOB PRIMARY KEY CHECK (length(id) = 16),
  ledger TEXT NOT NULL,
  wallet_address TEXT NOT NULL CHECK (length(wallet_address) BETWEEN 1 AND 2048),
  -- The client that made it, as grants name clients; NULL for the owner.
  client TEXT CHECK (client IS NULL OR length(client) BETWEEN 1 AND 2048),
  -- The incoming payment it pays, and how to reach it.
  receiver TEXT NOT NULL CHECK (length(receiver) BETWEEN 1 AND 2048),
  destination TEXT NOT NULL CHECK (length(destination) BETWEEN 1 AND 1023),
  shared_secret BLOB NOT NULL CHECK (length(shared_secret) = 32),
  debit_amount INTEGER NOT NULL CHECK (debit_amount > 0),
  receive_amount INTEGER NOT NULL CHECK (receive_amount > 0),
  receive_asset_code TEXT NOT NULL CHECK (length(receive_asset_code) BETWEEN 1 AND 32),
  receive_asset_scale INTEGER NOT NULL CHECK (receive_asset_scale BETWEEN 0 AND 255),
  created_at_ms INTEGER NOT NULL,
  expires_at_ms INTEGER NOT NULL CHECK (expires_at_ms > created_at_ms),
  -- The wallet address an owner's payment goes to, as the owner named it;
  -- NULL when only the incoming payment is known.
  recipient TEXT CHECK (recipient IS NULL OR length(recipient) BETWEEN 1 AND 2048),
  -- A payment of a fixed amount to send sends all of it; one of a fixed
  -- amount to deliver stops once the receiver has it.
  fixed_debit INTEGER NOT NULL DEFAULT 0 CHECK (fixed_debit IN (0, 1))
) STRICT, WITHOUT ROWID;

CREATE INDEX quotes_by_expiry ON quotes (expires_at_ms);

CREATE TABLE outgoing_payments (
  id BLOB PRIMARY KEY CHECK (length(id) = 16),
  ledger TEXT NOT NULL,
  wallet_address TEXT NOT NULL CHECK (length(wallet_address) BETWEEN 1 AND 2048),
  client TEXT CHECK (client IS NULL OR length(client) BETWEEN 1 AND 2048),
  -- The grant it spends, and the window of the grant's interval it was
  -- reserved in.
  grant_id BLOB CHECK (grant_id IS NULL OR length(grant_id) = 16),
  grant_window_ms INTEGER CHECK ((grant_id IS NULL) = (grant_window_ms IS NULL)),
  -- A quote pays one payment only.
  quote_id BLOB UNIQUE CHECK (quote_id IS NULL OR length(quote_id) = 16),
  receiver TEXT NOT NULL CHECK (length(receiver) BETWEEN 1 AND 2048),
  destination TEXT NOT NULL CHECK (length(destination) BETWEEN 1 AND 1023),
  shared_secret BLOB NOT NULL CHECK (length(shared_secret) = 32),
  debit_amount INTEGER NOT NULL CHECK (debit_amount > 0),
  receive_amount INTEGER NOT NULL CHECK (receive_amount > 0),
  receive_asset_code TEXT NOT NULL CHECK (length(receive_asset_code) BETWEEN 1 AND 32),
  receive_asset_scale INTEGER NOT NULL CHECK (receive_asset_scale BETWEEN 0 AND 255),
  -- What was posted: never more than the debit amount.
  sent_amount INTEGER NOT NULL CHECK (sent_amount >= 0 AND sent_amount <= debit_amount),
  state TEXT NOT NULL CHECK (state IN ('sending', 'completed', 'failed')),
  failure TEXT CHECK ((state = 'failed') = (failure IS NOT NULL) AND length(failure) <= 256),
  metadata TEXT CHECK (metadata IS NULL OR length(CAST(metadata AS BLOB)) <= 4096),
  created_at_ms INTEGER NOT NULL,
  -- When it fails if it has not sent all by then.
  deadline_ms INTEGER NOT NULL CHECK (deadline_ms > created_at_ms),
  finished_at_ms INTEGER CHECK ((state = 'sending') = (finished_at_ms IS NULL)),
  revision INTEGER NOT NULL UNIQUE CHECK (revision > 0),
  recipient TEXT CHECK (recipient IS NULL OR length(recipient) BETWEEN 1 AND 2048),
  fixed_debit INTEGER NOT NULL DEFAULT 0 CHECK (fixed_debit IN (0, 1))
) STRICT, WITHOUT ROWID;

CREATE INDEX outgoing_payments_by_ledger ON outgoing_payments (ledger, created_at_ms, id);
CREATE INDEX outgoing_payments_by_client ON outgoing_payments (ledger, client, created_at_ms, id);
CREATE INDEX outgoing_payments_by_time ON outgoing_payments (created_at_ms, id);
CREATE INDEX outgoing_payments_sending ON outgoing_payments (created_at_ms) WHERE state = 'sending';
-- An outgoing payment never uses another's credentials, which is looked up
-- by destination.
CREATE INDEX outgoing_payments_by_destination ON outgoing_payments (destination);

CREATE TABLE grant_spend (
  grant_id BLOB NOT NULL CHECK (length(grant_id) = 16),
  -- The start of the window of the grant's interval; 0 without one.
  window_start_ms INTEGER NOT NULL CHECK (window_start_ms >= 0),
  debit_amount INTEGER NOT NULL CHECK (debit_amount >= 0),
  receive_amount INTEGER NOT NULL CHECK (receive_amount >= 0),
  PRIMARY KEY (grant_id, window_start_ms)
) STRICT, WITHOUT ROWID;

CREATE TABLE payment_settings (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  -- The ledger `$host` names; NULL for the lowest active one.
  default_ledger TEXT,
  -- The currency totals are shown in.
  display_currency TEXT CHECK (display_currency IS NULL OR length(display_currency) BETWEEN 1 AND 8)
) STRICT;

INSERT INTO payment_settings VALUES (1, NULL, NULL);

-- Settlement hubs (spec/areas/hubs.md#in-the-node), each by the scheme
-- over it, which names the hub's asset and ledger: whether the node moves
-- funds between the owner book of the hub's asset and the hub by itself;
-- until when an owner's move pauses that; the move on its way, with the
-- counter value that shows it done and its payment, so that a restart never
-- pays it twice; and the move the owner gave up, while it holds the node's
-- own moves back. A hub without a row reads as one with nothing on its way
-- and the node's own moves on.
CREATE TABLE hub_funding (
  scheme TEXT NOT NULL PRIMARY KEY CHECK (length(scheme) BETWEEN 1 AND 64),
  automatic INTEGER NOT NULL CHECK (automatic IN (0, 1)),
  paused_until_ms INTEGER NOT NULL CHECK (paused_until_ms >= 0),
  move_kind TEXT CHECK (move_kind IN ('deposit', 'withdraw')),
  move_drops INTEGER CHECK (move_drops > 0),
  move_target INTEGER CHECK (move_target > 0),
  move_payment TEXT CHECK (length(move_payment) BETWEEN 1 AND 64),
  move_by_owner INTEGER CHECK (move_by_owner IN (0, 1)),
  move_sent_ms INTEGER CHECK (move_sent_ms >= 0),
  move_applied_ms INTEGER CHECK (move_applied_ms >= 0),
  given_up_kind TEXT CHECK (given_up_kind IN ('deposit', 'withdraw')),
  given_up_drops INTEGER CHECK (given_up_drops > 0),
  CHECK ((given_up_kind IS NULL) = (given_up_drops IS NULL)),
  CHECK ((move_kind IS NULL) = (move_drops IS NULL)
    AND (move_kind IS NULL) = (move_target IS NULL)
    AND (move_kind IS NULL) = (move_payment IS NULL)
    AND (move_kind IS NULL) = (move_by_owner IS NULL)
    AND (move_kind IS NULL) = (move_sent_ms IS NULL)
    AND (move_kind IS NOT NULL OR move_applied_ms IS NULL))
) STRICT, WITHOUT ROWID;

-- The settlement hubs the node validates
-- (spec/areas/hubs.md#the-validator-in-the-node), each by its ID: its
-- consensus state, the states after its recent final blocks and after the
-- last blocks of recent epochs, how far it followed the escrow on the XRP
-- Ledger and the newest ledger it saw there
-- (spec/decisions/ledger-resets.md), and the payout bundles it signed. The
-- states are the hub crate's encodings (`store/hubs.rs` has the bounds).
CREATE TABLE hub_consensus (
  hub BLOB PRIMARY KEY CHECK (length(hub) = 32),
  updated_at_ms INTEGER NOT NULL,
  state BLOB NOT NULL CHECK (length(state) BETWEEN 1 AND 536870912)
) STRICT, WITHOUT ROWID;

CREATE TABLE hub_states (
  hub BLOB NOT NULL CHECK (length(hub) = 32),
  height INTEGER NOT NULL CHECK (height >= 0),
  -- Whether the block is the last of an epoch, kept longer than the others.
  boundary INTEGER NOT NULL CHECK (boundary IN (0, 1)),
  state BLOB NOT NULL CHECK (length(state) BETWEEN 1 AND 268435456),
  PRIMARY KEY (hub, height)
) STRICT, WITHOUT ROWID;

CREATE TABLE hub_escrow (
  hub BLOB PRIMARY KEY CHECK (length(hub) = 32),
  newest_ledger INTEGER NOT NULL CHECK (newest_ledger BETWEEN 0 AND 4294967295),
  newest_close_time INTEGER NOT NULL CHECK (newest_close_time >= 0),
  scanned_to INTEGER NOT NULL CHECK (scanned_to BETWEEN 0 AND 4294967295)
) STRICT, WITHOUT ROWID;

CREATE TABLE hub_bundles (
  hub BLOB NOT NULL CHECK (length(hub) = 32),
  height INTEGER NOT NULL CHECK (height >= 0),
  signed_at_ms INTEGER NOT NULL,
  bundle BLOB NOT NULL CHECK (length(bundle) BETWEEN 1 AND 16777216),
  PRIMARY KEY (hub, height)
) STRICT, WITHOUT ROWID;

-- Founding settlement hubs (spec/areas/hubs.md#founding-a-hub): what a
-- validator keeps of each founding it took part in, by the terms' nonce,
-- the genesis it signed and whether it runs the hub; a sponsor's foundings,
-- their escrow's funding and why a sponsor stopped funding one for good;
-- and the payout bundles a member read from its hub's validators, the
-- newest of each.
CREATE TABLE hub_founding_records (
  nonce BLOB PRIMARY KEY CHECK (length(nonce) = 32),
  sponsor BLOB NOT NULL CHECK (length(sponsor) = 32),
  terms BLOB NOT NULL CHECK (length(terms) = 32),
  accepted_at_ms INTEGER NOT NULL CHECK (accepted_at_ms >= 0),
  rules BLOB CHECK (length(rules) BETWEEN 1 AND 65536),
  genesis_ms INTEGER CHECK (genesis_ms >= 0),
  running INTEGER NOT NULL CHECK (running IN (0, 1)),
  CHECK ((rules IS NULL) = (genesis_ms IS NULL)),
  CHECK (running = 0 OR rules IS NOT NULL)
) STRICT, WITHOUT ROWID;

CREATE TABLE hub_foundings (
  nonce BLOB PRIMARY KEY CHECK (length(nonce) = 32),
  state BLOB NOT NULL CHECK (length(state) BETWEEN 1 AND 65536),
  xrpl TEXT NOT NULL CHECK (length(xrpl) BETWEEN 1 AND 2048),
  funding_drops INTEGER NOT NULL CHECK (funding_drops > 0),
  escrow_payment TEXT CHECK (length(escrow_payment) BETWEEN 1 AND 64),
  problem TEXT CHECK (length(problem) BETWEEN 1 AND 512),
  created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
  updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0),
  escrow_stop TEXT CHECK (escrow_stop IN ('too-little', 'failed'))
) STRICT, WITHOUT ROWID;

CREATE TABLE hub_payouts (
  hub BLOB NOT NULL CHECK (length(hub) = 32),
  validator INTEGER NOT NULL CHECK (validator BETWEEN 0 AND 15),
  height INTEGER NOT NULL CHECK (height >= 0),
  received_at_ms INTEGER NOT NULL CHECK (received_at_ms >= 0),
  bundle BLOB NOT NULL CHECK (length(bundle) BETWEEN 1 AND 16777216),
  PRIMARY KEY (hub, validator)
) STRICT, WITHOUT ROWID;
";

/// Migration 2: a peering's settlement methods, a row each
/// (spec/decisions/settlement-methods.md): its scheme, its place in the
/// order the pair agreed them, the scheme's state for the peer, the credit
/// we extend on it, and how long the peer's room on it has been short. A
/// stored peering's one method moves out of its `peers` row, which keeps
/// what is the pair's. `ALTER TABLE ... DROP COLUMN` rebuilds `peers` in
/// place, so `peer_terminations` keeps its reference, and nothing cascades.
const MIGRATION_2: &str = "
CREATE TABLE peer_methods (
  node TEXT NOT NULL REFERENCES peers (node) ON DELETE CASCADE,
  scheme TEXT NOT NULL CHECK (length(scheme) BETWEEN 2 AND 64),
  position INTEGER NOT NULL CHECK (position BETWEEN 0 AND 15),
  state BLOB NOT NULL,
  credit_out INTEGER NOT NULL CHECK (credit_out >= 0),
  short_for_ms INTEGER NOT NULL CHECK (short_for_ms >= 0),
  short_reached_ms INTEGER NOT NULL
    CHECK (short_reached_ms BETWEEN 0 AND short_for_ms),
  PRIMARY KEY (node, scheme),
  UNIQUE (node, position)
) STRICT, WITHOUT ROWID;

INSERT INTO peer_methods (node, scheme, position, state, credit_out, short_for_ms,
  short_reached_ms)
SELECT node, scheme, 0, state, credit_out, short_for_ms, short_reached_ms FROM peers;

ALTER TABLE peers DROP COLUMN scheme;
ALTER TABLE peers DROP COLUMN state;
ALTER TABLE peers DROP COLUMN credit_out;
ALTER TABLE peers DROP COLUMN short_for_ms;
ALTER TABLE peers DROP COLUMN short_reached_ms;
";

/// Migration 3: winding settlement methods down
/// (spec/decisions/settlement-methods.md). A method the node decided to
/// drop, and collects on first, keeps since when; a scheme its owner
/// removes keeps since when, and runs until its methods are dropped. Both
/// carry on across a restart.
const MIGRATION_3: &str = "
ALTER TABLE peer_methods ADD COLUMN winding_since_ms INTEGER
  CHECK (winding_since_ms IS NULL OR winding_since_ms >= 0);

ALTER TABLE settlement_schemes ADD COLUMN removing_since_ms INTEGER
  CHECK (removing_since_ms IS NULL OR removing_since_ms >= 0);
";

/// Migration 4: what only settling directly needed goes
/// (spec/decisions/hubs-only.md). A settlement channel no longer keeps
/// whether we once ended a peering for want of room: over a hub the hold is
/// all the credit there is, so the mark withholds nothing, and the cooldown
/// after such an end is never persisted
/// (spec/decisions/ended-for-room.md). The indexes that found the fees a
/// peer earned us, and where to start reading them, go with the credit
/// that grew from them. The column's own CHECK names only it, so SQLite
/// drops it in place.
const MIGRATION_4: &str = "
ALTER TABLE settlement_channels DROP COLUMN ended_for_room;

DROP INDEX ledger_transfers_by_packet;
DROP INDEX ledger_transfers_trust;
";

/// Migration 5: a hub's validator keeps its verifier's state with how far
/// it followed the escrow (spec/areas/hubs.md#the-validator-in-the-node):
/// the hub's network, then the verifier's UNL lists and manifests, whether
/// it is UNL-blocked, and its anchor, as the verifier writes them
/// (spec/areas/xrpl.md#state-and-restarts), so that a restart neither
/// lifts a block nor takes an older anchor as current. It is bounded as an
/// XRPL account's state is, and NULL until the watcher first writes it.
const MIGRATION_5: &str = "
ALTER TABLE hub_escrow ADD COLUMN verifier BLOB
  CHECK (verifier IS NULL OR length(verifier) <= 262144);
";

/// Migration 6: a validator's record of a founding names its sponsor by
/// its identity, its Ed25519 key and its SLH-DSA key, as hub rules now do
/// (spec/decisions/post-quantum-node-id.md). A record kept before names
/// the Ed25519 key alone, with rules in the layout before, which no longer
/// decode, so it is dropped with the table its length check is part of. A
/// sponsor's foundings kept before hold terms in that layout too, which no
/// longer decode either, so they go as well.
const MIGRATION_6: &str = "
DELETE FROM hub_foundings;

DROP TABLE hub_founding_records;

CREATE TABLE hub_founding_records (
  nonce BLOB PRIMARY KEY CHECK (length(nonce) = 32),
  sponsor BLOB NOT NULL CHECK (length(sponsor) = 64),
  terms BLOB NOT NULL CHECK (length(terms) = 32),
  accepted_at_ms INTEGER NOT NULL CHECK (accepted_at_ms >= 0),
  rules BLOB CHECK (length(rules) BETWEEN 1 AND 65536),
  genesis_ms INTEGER CHECK (genesis_ms >= 0),
  running INTEGER NOT NULL CHECK (running IN (0, 1)),
  CHECK ((rules IS NULL) = (genesis_ms IS NULL)),
  CHECK (running = 0 OR rules IS NOT NULL)
) STRICT, WITHOUT ROWID;
";

/// Migration 7: a wallet address per asset
/// (spec/areas/payments.md#wallet-addresses). The default names the asset
/// of a wallet address, its name, no longer a ledger: a ledger kept as the
/// default becomes its asset's name, which a hub's ledger shares with the
/// owner book of its asset. Where the node keeps what it receives of each
/// asset over Interledger, in the asset's settlement hub or on the owner's
/// own account, is a row per asset the owner chose for; an asset without
/// one keeps it in the hub.
const MIGRATION_7: &str = "
ALTER TABLE payment_settings RENAME COLUMN default_ledger TO default_address;

UPDATE payment_settings SET default_address = CASE default_address
  WHEN 'stub+usd' THEN 'usd'
  WHEN 'xrpl-testnet-xrp+xrp' THEN 'xrp'
  WHEN 'xrpl-xrp+xrp' THEN 'xrp'
  WHEN 'hub-xrpl-testnet-xrp+xrp' THEN 'xrp'
  WHEN 'hub-xrpl-xrp+xrp' THEN 'xrp'
  WHEN 'xrpl-testnet-rlusd+usd' THEN 'rlusd'
  WHEN 'xrpl-rlusd+usd' THEN 'rlusd'
  WHEN 'hub-xrpl-rlusd+usd' THEN 'rlusd'
  WHEN 'xrpl-testnet-dsusd+usd' THEN 'dsusd'
  WHEN 'hub-xrpl-testnet-dsusd+usd' THEN 'dsusd'
  ELSE NULL END;

CREATE TABLE funds_kept (
  asset TEXT NOT NULL PRIMARY KEY CHECK (length(asset) BETWEEN 1 AND 16),
  kept TEXT NOT NULL CHECK (kept IN ('hub', 'own'))
) STRICT, WITHOUT ROWID;
";

/// Migration 8: whether the node offers swaps to its peers, and swaps
/// with them (spec/decisions/peer-swaps.md): both on, as for a new node.
const MIGRATION_8: &str = "
ALTER TABLE peer_settings ADD COLUMN offer_swaps INTEGER NOT NULL DEFAULT 1
  CHECK (offer_swaps IN (0, 1));

ALTER TABLE peer_settings ADD COLUMN swapping INTEGER NOT NULL DEFAULT 1
  CHECK (swapping IN (0, 1));
";

/// Migration 9: the targets a periphery node's own swaps aim for, per
/// asset, by the ledger its holding counts toward
/// (spec/decisions/peer-swaps.md): none, as for a new node.
const MIGRATION_9: &str = "
CREATE TABLE swap_targets (
  asset TEXT NOT NULL PRIMARY KEY CHECK (length(asset) BETWEEN 1 AND 64),
  target_ppm INTEGER NOT NULL CHECK (target_ppm BETWEEN 1 AND 1000000)
) STRICT, WITHOUT ROWID;
";

/// Migration 10: where the node keeps what it receives is no longer the
/// owner's choice: its hubs keep a float and the rest is withdrawn to the
/// owner's own account (spec/decisions/one-balance-per-asset.md).
const MIGRATION_10: &str = "
DROP TABLE funds_kept;
";

/// Migration 11: the target the owner set for what the node keeps in a
/// settlement hub, in the hub ledger's units
/// (spec/areas/hubs.md#in-the-node,
/// spec/decisions/generous-hub-target.md): NULL, the default, for every
/// hub kept before.
const MIGRATION_11: &str = "
ALTER TABLE hub_funding ADD COLUMN target INTEGER
  CHECK (target IS NULL OR target > 0);
";

/// Columns that hold secrets: the database inspector never reads them.
pub const SECRET_COLUMNS: &[(&str, &str)] = &[
    ("node_key", "private_key"),
    ("tls_certificate", "key_pem"),
    ("acme_account", "credentials"),
    ("sessions", "id"),
    ("btp_tokens", "token_hash"),
    ("incoming_payments", "shared_secret"),
    ("gnap_grants", "continue_token_hash"),
    ("gnap_grants", "interaction"),
    ("gnap_tokens", "token_hash"),
    ("quotes", "shared_secret"),
    ("outgoing_payments", "shared_secret"),
    ("setup_token", "token"),
    // With the name, it opens a commitment still sealed.
    ("name_registrations", "salt"),
];

/// A column as the code expects it: name, type, NOT NULL, part of the
/// primary key.
type Column = (&'static str, &'static str, bool, bool);

/// A table as the code expects it.
pub struct Table {
    /// Its name.
    pub name: &'static str,
    /// Whether it is a `WITHOUT ROWID` table.
    pub without_rowid: bool,
    /// Its columns: name, type, NOT NULL, part of the primary key.
    pub columns: &'static [Column],
}

/// Every table, as the code expects it.
pub const TABLES: &[Table] = &[
    Table {
        name: "node_config",
        without_rowid: false,
        columns: &[
            ("id", "INTEGER", false, true),
            ("realm", "TEXT", true, false),
            ("hostname", "TEXT", false, false),
            ("alias", "TEXT", true, false),
            ("acme_directory_url", "TEXT", true, false),
        ],
    },
    Table {
        name: "node_key",
        without_rowid: false,
        columns: &[
            ("id", "INTEGER", false, true),
            ("private_key", "BLOB", true, false),
            ("created_at_ms", "INTEGER", true, false),
        ],
    },
    Table {
        name: "tls_certificate",
        without_rowid: false,
        columns: &[
            ("id", "INTEGER", false, true),
            ("source", "TEXT", true, false),
            ("chain_pem", "TEXT", true, false),
            ("key_pem", "TEXT", true, false),
            ("installed_at_ms", "INTEGER", true, false),
        ],
    },
    Table {
        name: "acme_account",
        without_rowid: true,
        columns: &[
            ("directory_url", "TEXT", true, true),
            ("credentials", "TEXT", true, false),
            ("contact", "TEXT", true, false),
            ("created_at_ms", "INTEGER", true, false),
        ],
    },
    Table {
        name: "sessions",
        without_rowid: true,
        columns: &[
            ("id", "BLOB", true, true),
            ("created_at_ms", "INTEGER", true, false),
            ("last_used_at_ms", "INTEGER", true, false),
            ("user_agent", "TEXT", true, false),
        ],
    },
    Table {
        name: "devices",
        without_rowid: true,
        columns: &[
            ("id", "TEXT", true, true),
            ("name", "TEXT", true, false),
            ("paired_at_ms", "INTEGER", true, false),
            ("last_seen_at_ms", "INTEGER", true, false),
        ],
    },
    Table {
        name: "boot",
        without_rowid: false,
        columns: &[
            ("id", "INTEGER", false, true),
            ("epoch", "INTEGER", true, false),
        ],
    },
    Table {
        name: "ledger_accounts",
        without_rowid: false,
        columns: &[
            ("id", "INTEGER", false, true),
            ("path", "TEXT", true, false),
            ("debits_posted", "INTEGER", true, false),
            ("credits_posted", "INTEGER", true, false),
        ],
    },
    Table {
        name: "ledger_transfers",
        without_rowid: false,
        columns: &[
            ("id", "INTEGER", false, true),
            ("debit", "INTEGER", true, false),
            ("credit", "INTEGER", true, false),
            ("amount", "INTEGER", true, false),
            ("state", "TEXT", true, false),
            ("created_at_ms", "INTEGER", true, false),
            ("resolved_at_ms", "INTEGER", false, false),
            ("reference", "TEXT", true, false),
            ("packet_epoch", "INTEGER", false, false),
            ("packet_sequence", "INTEGER", false, false),
            ("settlement_scheme", "TEXT", false, false),
            ("settlement_id", "TEXT", false, false),
            ("sequence", "INTEGER", true, false),
        ],
    },
    Table {
        name: "settlement_schemes",
        without_rowid: true,
        columns: &[
            ("id", "TEXT", true, true),
            ("config", "TEXT", true, false),
            ("activated_at_ms", "INTEGER", true, false),
            ("removing_since_ms", "INTEGER", false, false),
        ],
    },
    Table {
        name: "settlement_checkpoints",
        without_rowid: true,
        columns: &[
            ("scheme", "TEXT", true, true),
            ("balance", "INTEGER", true, false),
            ("updated_at_ms", "INTEGER", true, false),
            ("state", "BLOB", false, false),
        ],
    },
    Table {
        name: "asset_checkpoints",
        without_rowid: true,
        columns: &[
            ("asset", "TEXT", true, true),
            ("updated_at_ms", "INTEGER", true, false),
            ("state", "BLOB", true, false),
        ],
    },
    Table {
        name: "asset_configs",
        without_rowid: true,
        columns: &[
            ("asset", "TEXT", true, true),
            ("config", "TEXT", true, false),
            ("updated_at_ms", "INTEGER", true, false),
        ],
    },
    Table {
        name: "btp_tokens",
        without_rowid: false,
        columns: &[
            ("id", "INTEGER", false, true),
            ("token_hash", "BLOB", true, false),
            ("label", "TEXT", true, false),
            ("ledger", "TEXT", true, false),
            ("created_at_ms", "INTEGER", true, false),
            ("revoked_at_ms", "INTEGER", false, false),
            ("cap_amount", "INTEGER", false, false),
            ("cap_window_ms", "INTEGER", false, false),
            ("cap_used_amount", "INTEGER", true, false),
            ("cap_used_remainder", "INTEGER", true, false),
            ("cap_as_of_ms", "INTEGER", true, false),
        ],
    },
    Table {
        name: "peer_nodes",
        without_rowid: true,
        columns: &[
            ("id", "TEXT", true, true),
            ("link_state", "BLOB", false, false),
            ("sources", "INTEGER", true, false),
            ("first_seen_ms", "INTEGER", true, false),
            ("registered_at_ms", "INTEGER", false, false),
            ("renewed_at_ms", "INTEGER", false, false),
            ("registration_expires_ms", "INTEGER", false, false),
            ("registered_url", "TEXT", false, false),
            ("registered_network", "BLOB", false, false),
        ],
    },
    Table {
        name: "peers",
        without_rowid: true,
        columns: &[
            ("node", "TEXT", true, true),
            ("since_ms", "INTEGER", true, false),
            ("inbound", "INTEGER", true, false),
            ("last_seen_ms", "INTEGER", true, false),
            ("depeering_since_ms", "INTEGER", false, false),
            ("depeering_reason", "INTEGER", false, false),
        ],
    },
    Table {
        name: "peer_methods",
        without_rowid: true,
        columns: &[
            ("node", "TEXT", true, true),
            ("scheme", "TEXT", true, true),
            ("position", "INTEGER", true, false),
            ("state", "BLOB", true, false),
            ("credit_out", "INTEGER", true, false),
            ("short_for_ms", "INTEGER", true, false),
            ("short_reached_ms", "INTEGER", true, false),
            ("winding_since_ms", "INTEGER", false, false),
        ],
    },
    Table {
        name: "own_link_state",
        without_rowid: false,
        columns: &[
            ("id", "INTEGER", false, true),
            ("sequence", "INTEGER", true, false),
        ],
    },
    Table {
        name: "peer_terminations",
        without_rowid: true,
        columns: &[
            ("node", "TEXT", true, true),
            ("since_ms", "INTEGER", true, false),
            ("reason", "INTEGER", true, false),
            ("by_us", "INTEGER", true, false),
            ("acknowledged", "INTEGER", true, false),
        ],
    },
    Table {
        name: "settlement_channels",
        without_rowid: true,
        columns: &[
            ("node", "TEXT", true, true),
            ("public_key", "BLOB", true, false),
            ("url", "TEXT", true, false),
            ("settled_in", "INTEGER", true, false),
            ("settled_out", "INTEGER", true, false),
            ("outbox_sequence", "INTEGER", false, false),
            ("outbox_scheme", "TEXT", false, false),
            ("outbox_amount", "INTEGER", false, false),
            ("outbox_data", "BLOB", false, false),
            ("outbox_attempts", "INTEGER", false, false),
            ("closed", "INTEGER", true, false),
            ("paid_us", "INTEGER", true, false),
        ],
    },
    Table {
        name: "peer_bans",
        without_rowid: true,
        columns: &[("node", "TEXT", true, true)],
    },
    Table {
        name: "name_log",
        without_rowid: false,
        columns: &[
            ("id", "INTEGER", false, true),
            ("next_position", "INTEGER", true, false),
            ("last_logged_ms", "INTEGER", true, false),
        ],
    },
    Table {
        name: "name_commitments",
        without_rowid: true,
        columns: &[
            ("commitment", "BLOB", true, true),
            ("position", "INTEGER", true, false),
            ("logged_ms", "INTEGER", true, false),
            ("window_end_ms", "INTEGER", true, false),
            ("committer", "TEXT", true, false),
            ("network", "BLOB", false, false),
            ("name", "TEXT", false, false),
            ("holder", "TEXT", false, false),
            ("contends", "INTEGER", false, false),
        ],
    },
    Table {
        name: "granted_names",
        without_rowid: true,
        columns: &[
            ("name", "TEXT", true, true),
            ("holder", "TEXT", true, false),
            ("granted_ms", "INTEGER", true, false),
            ("expires_ms", "INTEGER", true, false),
            ("freed_ms", "INTEGER", false, false),
        ],
    },
    Table {
        name: "name_holders",
        without_rowid: true,
        columns: &[
            ("name", "TEXT", true, true),
            ("holder", "TEXT", true, false),
            ("expires_ms", "INTEGER", true, false),
        ],
    },
    Table {
        name: "name_registrations",
        without_rowid: true,
        columns: &[
            ("name", "TEXT", true, true),
            ("salt", "BLOB", true, false),
            ("started_ms", "INTEGER", true, false),
            ("stage", "TEXT", true, false),
        ],
    },
    Table {
        name: "downlink_accounts",
        without_rowid: true,
        columns: &[("ledger", "TEXT", true, true), ("node", "TEXT", true, true)],
    },
    Table {
        name: "incoming_payments",
        without_rowid: true,
        columns: &[
            ("id", "BLOB", true, true),
            ("ledger", "TEXT", true, false),
            ("wallet_address", "TEXT", true, false),
            ("client", "TEXT", false, false),
            ("incoming_amount", "INTEGER", false, false),
            ("received_amount", "INTEGER", true, false),
            ("ilp_address", "TEXT", true, false),
            ("shared_secret", "BLOB", true, false),
            ("metadata", "TEXT", false, false),
            ("created_at_ms", "INTEGER", true, false),
            ("expires_at_ms", "INTEGER", true, false),
            ("completed_at_ms", "INTEGER", false, false),
            ("revision", "INTEGER", true, false),
        ],
    },
    Table {
        name: "gnap_grants",
        without_rowid: true,
        columns: &[
            ("id", "BLOB", true, true),
            ("state", "TEXT", true, false),
            ("client", "TEXT", true, false),
            ("client_key", "TEXT", true, false),
            ("access", "TEXT", true, false),
            ("needs_owner", "INTEGER", true, false),
            ("continue_token_hash", "BLOB", true, false),
            ("interaction", "TEXT", false, false),
            ("created_at_ms", "INTEGER", true, false),
            ("decided_at_ms", "INTEGER", false, false),
            ("expires_at_ms", "INTEGER", true, false),
            ("revision", "INTEGER", true, false),
            ("network", "BLOB", false, false),
        ],
    },
    Table {
        name: "gnap_tokens",
        without_rowid: true,
        columns: &[
            ("token_hash", "BLOB", true, true),
            ("grant_id", "BLOB", true, false),
            ("manage_id", "BLOB", true, false),
            ("created_at_ms", "INTEGER", true, false),
            ("expires_at_ms", "INTEGER", true, false),
        ],
    },
    Table {
        name: "quotes",
        without_rowid: true,
        columns: &[
            ("id", "BLOB", true, true),
            ("ledger", "TEXT", true, false),
            ("wallet_address", "TEXT", true, false),
            ("client", "TEXT", false, false),
            ("receiver", "TEXT", true, false),
            ("destination", "TEXT", true, false),
            ("shared_secret", "BLOB", true, false),
            ("debit_amount", "INTEGER", true, false),
            ("receive_amount", "INTEGER", true, false),
            ("receive_asset_code", "TEXT", true, false),
            ("receive_asset_scale", "INTEGER", true, false),
            ("created_at_ms", "INTEGER", true, false),
            ("expires_at_ms", "INTEGER", true, false),
            ("recipient", "TEXT", false, false),
            ("fixed_debit", "INTEGER", true, false),
        ],
    },
    Table {
        name: "outgoing_payments",
        without_rowid: true,
        columns: &[
            ("id", "BLOB", true, true),
            ("ledger", "TEXT", true, false),
            ("wallet_address", "TEXT", true, false),
            ("client", "TEXT", false, false),
            ("grant_id", "BLOB", false, false),
            ("grant_window_ms", "INTEGER", false, false),
            ("quote_id", "BLOB", false, false),
            ("receiver", "TEXT", true, false),
            ("destination", "TEXT", true, false),
            ("shared_secret", "BLOB", true, false),
            ("debit_amount", "INTEGER", true, false),
            ("receive_amount", "INTEGER", true, false),
            ("receive_asset_code", "TEXT", true, false),
            ("receive_asset_scale", "INTEGER", true, false),
            ("sent_amount", "INTEGER", true, false),
            ("state", "TEXT", true, false),
            ("failure", "TEXT", false, false),
            ("metadata", "TEXT", false, false),
            ("created_at_ms", "INTEGER", true, false),
            ("deadline_ms", "INTEGER", true, false),
            ("finished_at_ms", "INTEGER", false, false),
            ("revision", "INTEGER", true, false),
            ("recipient", "TEXT", false, false),
            ("fixed_debit", "INTEGER", true, false),
        ],
    },
    Table {
        name: "grant_spend",
        without_rowid: true,
        columns: &[
            ("grant_id", "BLOB", true, true),
            ("window_start_ms", "INTEGER", true, true),
            ("debit_amount", "INTEGER", true, false),
            ("receive_amount", "INTEGER", true, false),
        ],
    },
    Table {
        name: "payment_settings",
        without_rowid: false,
        columns: &[
            ("id", "INTEGER", false, true),
            ("default_address", "TEXT", false, false),
            ("display_currency", "TEXT", false, false),
        ],
    },
    Table {
        name: "peer_settings",
        without_rowid: false,
        columns: &[
            ("id", "INTEGER", false, true),
            ("connector_fee_ppm", "INTEGER", true, false),
            ("fx_spread_ppm", "INTEGER", true, false),
            ("rebalance_ppm", "INTEGER", true, false),
            ("offer_swaps", "INTEGER", true, false),
            ("swapping", "INTEGER", true, false),
        ],
    },
    Table {
        name: "swap_targets",
        without_rowid: true,
        columns: &[
            ("asset", "TEXT", true, true),
            ("target_ppm", "INTEGER", true, false),
        ],
    },
    Table {
        name: "rate_sources",
        without_rowid: false,
        columns: &[
            ("position", "INTEGER", false, true),
            ("kind", "TEXT", true, false),
            ("url", "TEXT", false, false),
        ],
    },
    Table {
        name: "exchange_rates",
        without_rowid: true,
        columns: &[
            ("asset", "TEXT", true, true),
            ("rate", "TEXT", true, false),
            ("as_of_ms", "INTEGER", true, false),
        ],
    },
    Table {
        name: "incoming_connections",
        without_rowid: true,
        columns: &[
            ("incoming_payment", "BLOB", true, true),
            ("connection", "BLOB", true, true),
            ("received", "INTEGER", true, false),
        ],
    },
    Table {
        name: "hub_funding",
        without_rowid: true,
        columns: &[
            ("scheme", "TEXT", true, true),
            ("automatic", "INTEGER", true, false),
            ("paused_until_ms", "INTEGER", true, false),
            ("move_kind", "TEXT", false, false),
            ("move_drops", "INTEGER", false, false),
            ("move_target", "INTEGER", false, false),
            ("move_payment", "TEXT", false, false),
            ("move_by_owner", "INTEGER", false, false),
            ("move_sent_ms", "INTEGER", false, false),
            ("move_applied_ms", "INTEGER", false, false),
            ("given_up_kind", "TEXT", false, false),
            ("given_up_drops", "INTEGER", false, false),
            ("target", "INTEGER", false, false),
        ],
    },
    Table {
        name: "setup_token",
        without_rowid: false,
        columns: &[
            ("id", "INTEGER", false, true),
            ("token", "BLOB", true, false),
        ],
    },
    Table {
        name: "hub_consensus",
        without_rowid: true,
        columns: &[
            ("hub", "BLOB", true, true),
            ("updated_at_ms", "INTEGER", true, false),
            ("state", "BLOB", true, false),
        ],
    },
    Table {
        name: "hub_states",
        without_rowid: true,
        columns: &[
            ("hub", "BLOB", true, true),
            ("height", "INTEGER", true, true),
            ("boundary", "INTEGER", true, false),
            ("state", "BLOB", true, false),
        ],
    },
    Table {
        name: "hub_escrow",
        without_rowid: true,
        columns: &[
            ("hub", "BLOB", true, true),
            ("newest_ledger", "INTEGER", true, false),
            ("newest_close_time", "INTEGER", true, false),
            ("scanned_to", "INTEGER", true, false),
            ("verifier", "BLOB", false, false),
        ],
    },
    Table {
        name: "hub_bundles",
        without_rowid: true,
        columns: &[
            ("hub", "BLOB", true, true),
            ("height", "INTEGER", true, true),
            ("signed_at_ms", "INTEGER", true, false),
            ("bundle", "BLOB", true, false),
        ],
    },
    Table {
        name: "hub_founding_records",
        without_rowid: true,
        columns: &[
            ("nonce", "BLOB", true, true),
            ("sponsor", "BLOB", true, false),
            ("terms", "BLOB", true, false),
            ("accepted_at_ms", "INTEGER", true, false),
            ("rules", "BLOB", false, false),
            ("genesis_ms", "INTEGER", false, false),
            ("running", "INTEGER", true, false),
        ],
    },
    Table {
        name: "hub_foundings",
        without_rowid: true,
        columns: &[
            ("nonce", "BLOB", true, true),
            ("state", "BLOB", true, false),
            ("xrpl", "TEXT", true, false),
            ("funding_drops", "INTEGER", true, false),
            ("escrow_payment", "TEXT", false, false),
            ("problem", "TEXT", false, false),
            ("created_at_ms", "INTEGER", true, false),
            ("updated_at_ms", "INTEGER", true, false),
            ("escrow_stop", "TEXT", false, false),
        ],
    },
    Table {
        name: "hub_payouts",
        without_rowid: true,
        columns: &[
            ("hub", "BLOB", true, true),
            ("validator", "INTEGER", true, true),
            ("height", "INTEGER", true, false),
            ("received_at_ms", "INTEGER", true, false),
            ("bundle", "BLOB", true, false),
        ],
    },
];

/// Claims a new file, refuses another application's or the early
/// testnet's, and runs `migrations` (this code's are [`MIGRATIONS`]).
pub fn prepare(connection: &mut Connection, migrations: &[&str]) -> Result<(), StoreError> {
    let application_id: i32 =
        connection.pragma_query_value(None, "application_id", |row| row.get(0))?;
    let schema_version: i32 =
        connection.pragma_query_value(None, "schema_version", |row| row.get(0))?;
    match application_id {
        APPLICATION_ID => {}
        0 if schema_version == 0 => {
            tracing::info!(target: crate::log::target::DATABASE, "initializing a new database");
            connection.pragma_update(None, "application_id", APPLICATION_ID)?;
        }
        EARLY_TESTNET_APPLICATION_ID => return Err(StoreError::EarlyTestnetDatabase),
        other => return Err(StoreError::ForeignDatabase(other)),
    }
    migrate(connection, migrations)
}

/// The schema version of a Dassie database that `migrations` will migrate,
/// if it is one: neither new, nor current, nor too new, nor foreign.
pub fn pending_migration(
    connection: &Connection,
    migrations: &[&str],
) -> Result<Option<i64>, StoreError> {
    let application_id: i32 =
        connection.pragma_query_value(None, "application_id", |row| row.get(0))?;
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    Ok(
        (application_id == APPLICATION_ID && version > 0 && version < version_of(migrations))
            .then_some(version),
    )
}

/// Runs those of `migrations` the database has not run, each in its own
/// transaction.
fn migrate(connection: &mut Connection, migrations: &[&str]) -> Result<(), StoreError> {
    let current: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let known = version_of(migrations);
    if current > known {
        return Err(StoreError::TooNew {
            found: current,
            known,
        });
    }
    for (index, sql) in migrations
        .iter()
        .enumerate()
        .skip(usize::try_from(current).unwrap_or(0))
    {
        let version = i64::try_from(index).unwrap_or(i64::MAX).saturating_add(1);
        let transaction = connection.transaction()?;
        transaction.execute_batch(sql)?;
        transaction.pragma_update(None, "user_version", version)?;
        transaction.commit()?;
        tracing::info!(target: crate::log::target::DATABASE, version, "migrated");
    }
    Ok(())
}

/// Checks every table against the code: present, STRICT, the right ROWID
/// setting, and exactly the expected columns.
pub fn check(connection: &mut Connection) -> Result<(), StoreError> {
    let transaction = connection.transaction()?;
    for table in TABLES {
        check_table(&transaction, table)?;
    }
    Ok(())
}

fn check_table(transaction: &Transaction<'_>, table: &Table) -> Result<(), StoreError> {
    let mismatch = |problem: &str| StoreError::SchemaMismatch(format!("{}: {problem}", table.name));
    let flags: Option<(i64, i64)> = transaction
        .query_row(
            "SELECT strict, wr FROM pragma_table_list WHERE schema = 'main' AND name = ?1",
            [table.name],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((strict, without_rowid)) = flags else {
        return Err(mismatch("missing"));
    };
    if strict != 1 {
        return Err(mismatch("not STRICT"));
    }
    if (without_rowid == 1) != table.without_rowid {
        return Err(mismatch("wrong WITHOUT ROWID setting"));
    }
    let mut statement = transaction
        .prepare("SELECT name, type, \"notnull\", pk FROM pragma_table_info(?1) ORDER BY cid")?;
    let columns = statement
        .query_map([table.name], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, bool>(2)?,
                row.get::<_, i64>(3)? > 0,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let expected = table
        .columns
        .iter()
        .map(|&(name, kind, not_null, pk)| (name.to_string(), kind.to_string(), not_null, pk));
    if !columns.iter().cloned().eq(expected) {
        return Err(mismatch(&format!("columns are {columns:?}")));
    }
    Ok(())
}
