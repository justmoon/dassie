//! Schema v1, its migrations, and the check that the file matches it.
//!
//! The original's database is not read (D3). The new one keeps its ideas: an
//! application ID that marks the file as Dassie's, `user_version`
//! migrations each in a transaction, STRICT tables, and a check at every
//! open that the tables are what the code expects (spec 2 §5.1). The check
//! is stricter than the original's: an extra column is an error too.

use rusqlite::{Connection, OptionalExtension, Transaction};

use super::StoreError;

/// Marks a file as a Dassie database. Like the original's, a random
/// positive int32 with nibbles 2–4 set to `DA5`.
pub const APPLICATION_ID: i32 = 0x7DA5_E2C9;

/// The original's ID. A file with it is a v0 database, which this version
/// does not read (D3).
pub const V0_APPLICATION_ID: i32 = 0x1DA5_3B81;

/// The migrations, in order. Migration `n` (1-based) brings `user_version`
/// to `n`. Never edit one that has shipped; add another.
const MIGRATIONS: &[&str] = &[
    MIGRATION_1,
    MIGRATION_2,
    MIGRATION_3,
    MIGRATION_4,
    MIGRATION_5,
    MIGRATION_6,
    MIGRATION_7,
    MIGRATION_8,
    MIGRATION_9,
    MIGRATION_10,
];

const MIGRATION_1: &str = "
CREATE TABLE node_config (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  realm TEXT NOT NULL CHECK (realm IN ('test', 'live')),
  hostname TEXT CHECK (hostname IS NULL OR (length(hostname) BETWEEN 1 AND 253 AND hostname = lower(hostname))),
  http_port INTEGER NOT NULL CHECK (http_port BETWEEN 1 AND 65535),
  https_port INTEGER NOT NULL CHECK (https_port BETWEEN 1 AND 65535 AND https_port != http_port),
  alias TEXT NOT NULL CHECK (length(CAST(alias AS BLOB)) <= 64),
  acme_directory_url TEXT NOT NULL CHECK (acme_directory_url LIKE 'https://%')
) STRICT;

INSERT INTO node_config VALUES
  (1, 'test', NULL, 80, 443, '', 'https://acme-v02.api.letsencrypt.org/directory');

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
";

/// The ledger (spec 7 §4.4): its accounts with their posted totals cached,
/// the journal of transfers, the settlement schemes and their checkpoints,
/// and the BTP tokens, by hash (D10). Amounts are integers in the ledger's
/// units; the ledger keeps every total below `i64::MAX`.
const MIGRATION_2: &str = "
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
    ('packet', 'settlement', 'trust', 'deposit', 'withdrawal', 'reconciliation')),
  packet_epoch INTEGER CHECK ((reference = 'packet') = (packet_epoch IS NOT NULL)),
  packet_sequence INTEGER CHECK ((reference = 'packet') = (packet_sequence IS NOT NULL)),
  settlement_scheme TEXT CHECK ((reference = 'settlement') = (settlement_scheme IS NOT NULL)),
  settlement_id TEXT CHECK ((reference = 'settlement') = (settlement_id IS NOT NULL)),
  -- The order rows were written or last changed in, for readers that page
  -- through the journal: a packet's transfer is written when it is posted,
  -- which is not the order of transfer IDs.
  sequence INTEGER NOT NULL UNIQUE CHECK (sequence > 0)
) STRICT;

CREATE INDEX ledger_transfers_by_debit ON ledger_transfers (debit, state);
CREATE INDEX ledger_transfers_by_credit ON ledger_transfers (credit, state);
CREATE INDEX ledger_transfers_pending ON ledger_transfers (state) WHERE state = 'pending';

CREATE TABLE settlement_schemes (
  id TEXT PRIMARY KEY,
  config TEXT NOT NULL,
  activated_at_ms INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

CREATE TABLE settlement_checkpoints (
  scheme TEXT PRIMARY KEY,
  balance INTEGER NOT NULL CHECK (balance >= 0),
  updated_at_ms INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

CREATE TABLE btp_tokens (
  id INTEGER PRIMARY KEY CHECK (id >= 1),
  token_hash BLOB NOT NULL UNIQUE CHECK (length(token_hash) = 32),
  label TEXT NOT NULL CHECK (length(CAST(label AS BLOB)) <= 64),
  ledger TEXT NOT NULL,
  created_at_ms INTEGER NOT NULL,
  revoked_at_ms INTEGER
) STRICT;
";

/// The peer protocol (spec 7 §4.4, §4.7): the node table, with each node's
/// verified link state, its registration with us and our peering with it;
/// and our own link state's sequence, which only goes up (NET-7).
const MIGRATION_3: &str = "
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

CREATE TABLE peers (
  node TEXT PRIMARY KEY REFERENCES peer_nodes (id) ON DELETE CASCADE,
  scheme TEXT NOT NULL,
  version INTEGER NOT NULL CHECK (version BETWEEN 0 AND 65535),
  state BLOB NOT NULL,
  credit_in INTEGER NOT NULL CHECK (credit_in >= 0),
  credit_out INTEGER NOT NULL CHECK (credit_out >= 0),
  since_ms INTEGER NOT NULL,
  inbound INTEGER NOT NULL CHECK (inbound IN (0, 1)),
  last_seen_ms INTEGER NOT NULL,
  -- The newest timestamp accepted from the peer: after a restart, nothing
  -- at or below it is accepted again.
  replay_floor_ms INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

CREATE TABLE own_link_state (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  sequence INTEGER NOT NULL CHECK (sequence >= 0)
) STRICT;

INSERT INTO own_link_state VALUES (1, 0);
";

/// The peering lifecycle and settlement (spec 7 §4.4, §4.7): peerings
/// being ended, settlement channels, which outlive peerings, and bans.
const MIGRATION_4: &str = "
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
  -- A settlement of everything owed, due since a peering ended: its scheme
  -- and the scheme's state for the peer.
  final_scheme TEXT,
  final_state BLOB CHECK ((final_scheme IS NULL) = (final_state IS NULL)),
  CHECK ((outbox_sequence IS NULL) = (outbox_scheme IS NULL)
     AND (outbox_scheme IS NULL) = (outbox_amount IS NULL)
     AND (outbox_amount IS NULL) = (outbox_data IS NULL)
     AND (outbox_data IS NULL) = (outbox_attempts IS NULL))
) STRICT, WITHOUT ROWID;

CREATE TABLE peer_bans (
  node TEXT PRIMARY KEY CHECK (length(node) BETWEEN 2 AND 45)
) STRICT, WITHOUT ROWID;
";

/// BTP spending caps (spec 7 §4.5), and the journal by account for the
/// wallet's account pages. `btp_tokens` is rebuilt, since its new CHECKs
/// span columns; nothing references it.
const MIGRATION_5: &str = "
CREATE TABLE btp_tokens_new (
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

INSERT INTO btp_tokens_new (id, token_hash, label, ledger, created_at_ms, revoked_at_ms)
  SELECT id, token_hash, label, ledger, created_at_ms, revoked_at_ms FROM btp_tokens;
DROP TABLE btp_tokens;
ALTER TABLE btp_tokens_new RENAME TO btp_tokens;

CREATE INDEX ledger_transfers_by_debit_sequence ON ledger_transfers (debit, sequence);
CREATE INDEX ledger_transfers_by_credit_sequence ON ledger_transfers (credit, sequence);
";

/// Open Payments (D9, spec 7 §4.4): incoming payments, written by the
/// engine with the journal rows that pay them; GNAP grants and their
/// access tokens, by hash (D10), written by the authorization server; and
/// which ledger `$host` names.
const MIGRATION_6: &str = "
CREATE TABLE incoming_payments (
  id BLOB PRIMARY KEY CHECK (length(id) = 16),
  ledger TEXT NOT NULL,
  wallet_address TEXT NOT NULL CHECK (length(wallet_address) BETWEEN 1 AND 2048),
  -- The client that created it, as grants name clients; NULL for the owner.
  client TEXT CHECK (client IS NULL OR length(client) BETWEEN 1 AND 2048),
  incoming_amount INTEGER CHECK (incoming_amount IS NULL OR incoming_amount > 0),
  received_amount INTEGER NOT NULL CHECK (received_amount >= 0
    AND (incoming_amount IS NULL OR received_amount <= incoming_amount)),
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
  revision INTEGER NOT NULL UNIQUE CHECK (revision > 0)
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

CREATE TABLE payment_settings (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  -- The ledger `$host` names; NULL for the lowest active one.
  default_ledger TEXT
) STRICT;

INSERT INTO payment_settings VALUES (1, NULL);
";

/// Verdicts on Fulfills between peers (D17, spec 7 §4.4): the Fulfills we
/// sent peers and wait for a verdict on, written with the journal rows that
/// post their packets; the Fulfills we took from peers, kept 30 days to
/// answer a resend the same way; and refusals in the journal, the
/// compensation booked when a peer refused a Fulfill, with the packet's
/// trace.
const MIGRATION_7: &str = "
CREATE TABLE ledger_transfers_new (
  id INTEGER PRIMARY KEY CHECK (id >= 0),
  debit INTEGER NOT NULL REFERENCES ledger_accounts (id),
  credit INTEGER NOT NULL REFERENCES ledger_accounts (id) CHECK (credit != debit),
  amount INTEGER NOT NULL CHECK (amount > 0),
  state TEXT NOT NULL CHECK (state IN ('pending', 'posted', 'voided')),
  created_at_ms INTEGER NOT NULL,
  resolved_at_ms INTEGER CHECK ((state = 'pending') = (resolved_at_ms IS NULL)),
  reference TEXT NOT NULL CHECK (reference IN
    ('packet', 'settlement', 'trust', 'deposit', 'withdrawal', 'reconciliation', 'refusal')),
  packet_epoch INTEGER CHECK ((reference IN ('packet', 'refusal')) = (packet_epoch IS NOT NULL)),
  packet_sequence INTEGER CHECK ((reference IN ('packet', 'refusal')) = (packet_sequence IS NOT NULL)),
  settlement_scheme TEXT CHECK ((reference = 'settlement') = (settlement_scheme IS NOT NULL)),
  settlement_id TEXT CHECK ((reference = 'settlement') = (settlement_id IS NOT NULL)),
  -- The order rows were written or last changed in, for readers that page
  -- through the journal: a packet's transfer is written when it is posted,
  -- which is not the order of transfer IDs.
  sequence INTEGER NOT NULL UNIQUE CHECK (sequence > 0)
) STRICT;

INSERT INTO ledger_transfers_new SELECT * FROM ledger_transfers;
DROP TABLE ledger_transfers;
ALTER TABLE ledger_transfers_new RENAME TO ledger_transfers;

CREATE INDEX ledger_transfers_by_debit ON ledger_transfers (debit, state);
CREATE INDEX ledger_transfers_by_credit ON ledger_transfers (credit, state);
CREATE INDEX ledger_transfers_pending ON ledger_transfers (state) WHERE state = 'pending';
CREATE INDEX ledger_transfers_by_debit_sequence ON ledger_transfers (debit, sequence);
CREATE INDEX ledger_transfers_by_credit_sequence ON ledger_transfers (credit, sequence);
CREATE INDEX ledger_transfers_refusals ON ledger_transfers (credit, created_at_ms)
  WHERE reference = 'refusal';

CREATE TABLE owed_fulfills (
  -- The packet, by its trace.
  packet_epoch INTEGER NOT NULL CHECK (packet_epoch >= 0),
  packet_sequence INTEGER NOT NULL CHECK (packet_sequence >= 0),
  -- The upstream peer, and the request ID its Prepare came with.
  peer TEXT NOT NULL CHECK (length(peer) BETWEEN 2 AND 45),
  request_id INTEGER NOT NULL CHECK (request_id BETWEEN 0 AND 4294967295),
  fulfill BLOB NOT NULL CHECK (length(fulfill) BETWEEN 1 AND 32806),
  -- What a refusal books: the packet reversed, when our own receiver took
  -- it, or a loss, when someone we cannot take it back from did.
  compensation TEXT NOT NULL CHECK (compensation IN ('reverse', 'loss')),
  -- The packet's transfers as posted; the first debits the peer's account.
  debit_1 INTEGER NOT NULL REFERENCES ledger_accounts (id),
  credit_1 INTEGER NOT NULL REFERENCES ledger_accounts (id),
  amount_1 INTEGER NOT NULL CHECK (amount_1 > 0),
  debit_2 INTEGER REFERENCES ledger_accounts (id),
  credit_2 INTEGER REFERENCES ledger_accounts (id),
  amount_2 INTEGER CHECK (amount_2 IS NULL OR amount_2 > 0),
  -- The incoming payment our receiver paid, and how much, which counts
  -- once the verdict says so.
  incoming_payment BLOB CHECK (incoming_payment IS NULL OR length(incoming_payment) = 16),
  incoming_amount INTEGER CHECK (incoming_amount IS NULL OR incoming_amount > 0),
  -- The expiry of the Prepare we sent on, and when the packet was posted.
  expires_at_ms INTEGER NOT NULL,
  created_at_ms INTEGER NOT NULL,
  CHECK ((debit_2 IS NULL) = (credit_2 IS NULL) AND (credit_2 IS NULL) = (amount_2 IS NULL)),
  CHECK ((incoming_payment IS NULL) = (incoming_amount IS NULL)),
  PRIMARY KEY (packet_epoch, packet_sequence)
) STRICT, WITHOUT ROWID;

CREATE TABLE peer_fulfills (
  -- The downstream peer, the request ID we sent it the Prepare with, and
  -- the packet's condition.
  peer TEXT NOT NULL CHECK (length(peer) BETWEEN 2 AND 45),
  request_id INTEGER NOT NULL CHECK (request_id BETWEEN 0 AND 4294967295),
  condition BLOB NOT NULL CHECK (length(condition) = 32),
  taken_at_ms INTEGER NOT NULL,
  PRIMARY KEY (peer, request_id, condition)
) STRICT, WITHOUT ROWID;

CREATE INDEX peer_fulfills_by_time ON peer_fulfills (taken_at_ms);

-- What arrived for an incoming payment and was taken back when a peer
-- refused its Fulfill: its STREAM connections still count it, so what they
-- report only grows.
ALTER TABLE incoming_payments ADD COLUMN released_amount INTEGER NOT NULL DEFAULT 0
  CHECK (released_amount >= 0);
";

/// Spending through Open Payments (D9, spec 7 §4.4): quotes, which the
/// resource server writes as it makes them; outgoing payments, written by
/// the engine with the journal rows that move their money; and what each
/// grant spent per window of its interval, written with the payments that
/// spend it.
const MIGRATION_8: &str = "
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
  expires_at_ms INTEGER NOT NULL CHECK (expires_at_ms > created_at_ms)
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
  revision INTEGER NOT NULL UNIQUE CHECK (revision > 0)
) STRICT, WITHOUT ROWID;

CREATE INDEX outgoing_payments_by_ledger ON outgoing_payments (ledger, created_at_ms, id);
CREATE INDEX outgoing_payments_by_client ON outgoing_payments (ledger, client, created_at_ms, id);
CREATE INDEX outgoing_payments_by_time ON outgoing_payments (created_at_ms, id);
CREATE INDEX outgoing_payments_sending ON outgoing_payments (created_at_ms) WHERE state = 'sending';

-- What a refusal took back is no longer counted: the payment's STREAM
-- connections are closed instead, so senders start anew (D17).
ALTER TABLE incoming_payments DROP COLUMN released_amount;

CREATE TABLE grant_spend (
  grant_id BLOB NOT NULL CHECK (length(grant_id) = 16),
  -- The start of the window of the grant's interval; 0 without one.
  window_start_ms INTEGER NOT NULL CHECK (window_start_ms >= 0),
  debit_amount INTEGER NOT NULL CHECK (debit_amount >= 0),
  receive_amount INTEGER NOT NULL CHECK (receive_amount >= 0),
  PRIMARY KEY (grant_id, window_start_ms)
) STRICT, WITHOUT ROWID;
";

const MIGRATION_9: &str = "
-- The wallet address an owner's payment goes to, as the owner named it;
-- NULL when only the incoming payment is known (D9).
ALTER TABLE quotes ADD COLUMN recipient TEXT
  CHECK (recipient IS NULL OR length(recipient) BETWEEN 1 AND 2048);
ALTER TABLE outgoing_payments ADD COLUMN recipient TEXT
  CHECK (recipient IS NULL OR length(recipient) BETWEEN 1 AND 2048);
";

/// One grace limit per peer (D18): the share of the credit we extend that
/// a peer may cost us before it is banned, and why each node was banned.
/// A record outlives its ban, so a peer unbanned is held to account only
/// for what it cost since.
const MIGRATION_10: &str = "
CREATE TABLE peer_settings (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  grace_share_ppm INTEGER NOT NULL CHECK (grace_share_ppm BETWEEN 1 AND 10000000)
) STRICT;

INSERT INTO peer_settings VALUES (1, 750000);

CREATE TABLE peer_ban_records (
  node TEXT PRIMARY KEY CHECK (length(node) BETWEEN 2 AND 45),
  reason TEXT NOT NULL CHECK (reason IN ('owner', 'grace-limit')),
  banned_at_ms INTEGER NOT NULL CHECK (banned_at_ms >= 0),
  -- What the peer cost, for a ban over the grace limit.
  cost_ledger TEXT CHECK (cost_ledger IS NULL OR length(cost_ledger) BETWEEN 1 AND 32),
  cost_refused INTEGER CHECK (cost_refused IS NULL OR cost_refused >= 0),
  cost_overdue INTEGER CHECK (cost_overdue IS NULL OR cost_overdue >= 0),
  cost_grace INTEGER CHECK (cost_grace IS NULL OR cost_grace >= 0),
  unbanned_at_ms INTEGER CHECK (unbanned_at_ms IS NULL OR unbanned_at_ms >= banned_at_ms),
  CHECK ((cost_ledger IS NULL) = (cost_refused IS NULL)
     AND (cost_refused IS NULL) = (cost_overdue IS NULL)
     AND (cost_overdue IS NULL) = (cost_grace IS NULL)),
  CHECK ((reason = 'grace-limit') = (cost_ledger IS NOT NULL))
) STRICT, WITHOUT ROWID;

CREATE INDEX peer_ban_records_unbanned ON peer_ban_records (unbanned_at_ms)
  WHERE unbanned_at_ms IS NOT NULL;
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
            ("http_port", "INTEGER", true, false),
            ("https_port", "INTEGER", true, false),
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
        ],
    },
    Table {
        name: "settlement_checkpoints",
        without_rowid: true,
        columns: &[
            ("scheme", "TEXT", true, true),
            ("balance", "INTEGER", true, false),
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
            ("scheme", "TEXT", true, false),
            ("version", "INTEGER", true, false),
            ("state", "BLOB", true, false),
            ("credit_in", "INTEGER", true, false),
            ("credit_out", "INTEGER", true, false),
            ("since_ms", "INTEGER", true, false),
            ("inbound", "INTEGER", true, false),
            ("last_seen_ms", "INTEGER", true, false),
            ("replay_floor_ms", "INTEGER", true, false),
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
            ("final_scheme", "TEXT", false, false),
            ("final_state", "BLOB", false, false),
        ],
    },
    Table {
        name: "peer_bans",
        without_rowid: true,
        columns: &[("node", "TEXT", true, true)],
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
        name: "owed_fulfills",
        without_rowid: true,
        columns: &[
            ("packet_epoch", "INTEGER", true, true),
            ("packet_sequence", "INTEGER", true, true),
            ("peer", "TEXT", true, false),
            ("request_id", "INTEGER", true, false),
            ("fulfill", "BLOB", true, false),
            ("compensation", "TEXT", true, false),
            ("debit_1", "INTEGER", true, false),
            ("credit_1", "INTEGER", true, false),
            ("amount_1", "INTEGER", true, false),
            ("debit_2", "INTEGER", false, false),
            ("credit_2", "INTEGER", false, false),
            ("amount_2", "INTEGER", false, false),
            ("incoming_payment", "BLOB", false, false),
            ("incoming_amount", "INTEGER", false, false),
            ("expires_at_ms", "INTEGER", true, false),
            ("created_at_ms", "INTEGER", true, false),
        ],
    },
    Table {
        name: "peer_fulfills",
        without_rowid: true,
        columns: &[
            ("peer", "TEXT", true, true),
            ("request_id", "INTEGER", true, true),
            ("condition", "BLOB", true, true),
            ("taken_at_ms", "INTEGER", true, false),
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
            ("default_ledger", "TEXT", false, false),
        ],
    },
    Table {
        name: "peer_settings",
        without_rowid: false,
        columns: &[
            ("id", "INTEGER", false, true),
            ("grace_share_ppm", "INTEGER", true, false),
        ],
    },
    Table {
        name: "peer_ban_records",
        without_rowid: true,
        columns: &[
            ("node", "TEXT", true, true),
            ("reason", "TEXT", true, false),
            ("banned_at_ms", "INTEGER", true, false),
            ("cost_ledger", "TEXT", false, false),
            ("cost_refused", "INTEGER", false, false),
            ("cost_overdue", "INTEGER", false, false),
            ("cost_grace", "INTEGER", false, false),
            ("unbanned_at_ms", "INTEGER", false, false),
        ],
    },
];

/// Claims a new file, refuses foreign or v0 ones, and migrates.
pub fn prepare(connection: &mut Connection) -> Result<(), StoreError> {
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
        V0_APPLICATION_ID => return Err(StoreError::V0Database),
        other => return Err(StoreError::ForeignDatabase(other)),
    }
    migrate(connection)
}

fn migrate(connection: &mut Connection) -> Result<(), StoreError> {
    migrate_until(connection, MIGRATIONS.len())
}

/// Runs the migrations up to `last`, for tests of an upgrade.
pub(super) fn migrate_until(connection: &mut Connection, last: usize) -> Result<(), StoreError> {
    let current: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let known = i64::try_from(MIGRATIONS.len()).unwrap_or(i64::MAX);
    if current > known {
        return Err(StoreError::TooNew {
            found: current,
            known,
        });
    }
    for (index, sql) in MIGRATIONS
        .iter()
        .enumerate()
        .take(last)
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
