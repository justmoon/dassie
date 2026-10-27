//! Schema v1, its migrations, and the check that the file matches it.
//!
//! The original's database is not read
//! (spec/decisions/database-continuity.md). The new one keeps its ideas: an
//! application ID that marks the file as Dassie's, `user_version` migrations
//! each in a transaction, STRICT tables, and a check at every open that the
//! tables are what the code expects (spec/areas/node.md#database). The check
//! is stricter than the original's: an extra column is an error too.

use rusqlite::{Connection, OptionalExtension, Transaction};

use super::StoreError;

/// Marks a file as a Dassie database. Like the original's, a random
/// positive int32 with nibbles 2–4 set to `DA5`.
pub const APPLICATION_ID: i32 = 0x7DA5_E2C9;

/// The original's ID. A file with it is a v0 database, which this version
/// does not read (spec/decisions/database-continuity.md).
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
    MIGRATION_11,
    MIGRATION_12,
    MIGRATION_13,
    MIGRATION_14,
    MIGRATION_15,
    MIGRATION_16,
    MIGRATION_17,
    MIGRATION_18,
    MIGRATION_19,
    MIGRATION_20,
    MIGRATION_21,
    MIGRATION_22,
    MIGRATION_23,
    MIGRATION_24,
    MIGRATION_25,
    MIGRATION_26,
    MIGRATION_27,
    MIGRATION_28,
    MIGRATION_29,
    MIGRATION_30,
    MIGRATION_31,
    MIGRATION_32,
    MIGRATION_33,
    MIGRATION_34,
    MIGRATION_35,
    MIGRATION_36,
    MIGRATION_37,
    MIGRATION_38,
    MIGRATION_39,
    MIGRATION_40,
    MIGRATION_41,
    MIGRATION_42,
    MIGRATION_43,
    MIGRATION_44,
    MIGRATION_45,
    MIGRATION_46,
    MIGRATION_47,
    MIGRATION_48,
    MIGRATION_49,
];

/// The schema version this code writes: the number of migrations.
// A handful of migrations: the length never comes near `i64::MAX`.
#[allow(clippy::cast_possible_wrap)]
pub const SCHEMA_VERSION: i64 = MIGRATIONS.len() as i64;

/// A STREAM connection per payment: each read of an incoming payment gets
/// credentials of its own, and what each connection received is kept apart,
/// so a sender learns what it delivered, not what others did. A Fulfill
/// owed a verdict names its connection. Those issued before had none: they
/// are one connection, whose count is what their payment received. An
/// outgoing payment never uses another's credentials, which is looked up
/// by destination.
const MIGRATION_13: &str = "
CREATE TABLE incoming_connections (
  incoming_payment BLOB NOT NULL REFERENCES incoming_payments (id) ON DELETE CASCADE,
  -- Which credentials of the payment the connection uses; zeros for those
  -- issued with the payment before connections had keys.
  connection BLOB NOT NULL CHECK (length(connection) = 16),
  received INTEGER NOT NULL CHECK (received > 0),
  PRIMARY KEY (incoming_payment, connection)
) STRICT, WITHOUT ROWID;

INSERT INTO incoming_connections
  SELECT id, zeroblob(16), received_amount FROM incoming_payments
  WHERE received_amount > 0 AND completed_at_ms IS NULL;

ALTER TABLE owed_fulfills ADD COLUMN incoming_connection BLOB
  CHECK (incoming_connection IS NULL OR length(incoming_connection) = 16);
UPDATE owed_fulfills SET incoming_connection = zeroblob(16) WHERE incoming_payment IS NOT NULL;

CREATE INDEX outgoing_payments_by_destination ON outgoing_payments (destination);
";

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

/// The ledger (spec/areas/node.md#persistence): its accounts with their
/// posted totals cached, the journal of transfers, the settlement schemes and
/// their checkpoints, and the BTP tokens, by hash
/// (spec/decisions/secrets-at-rest.md). Amounts are integers in the ledger's
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

/// The peer protocol (spec/areas/node.md#persistence): the node table, with
/// each node's verified link state, its registration with us and our peering
/// with it; and our own link state's sequence, which only goes up.
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

/// The peering lifecycle and settlement (spec/areas/node.md#persistence):
/// peerings being ended, settlement channels, which outlive peerings, and
/// bans.
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

/// BTP spending caps (spec/areas/connector.md#spending-caps), and the journal
/// by account for the wallet's account pages. `btp_tokens` is rebuilt, since
/// its new CHECKs span columns; nothing references it.
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

/// Open Payments (spec/decisions/open-payments.md,
/// spec/areas/payments.md#persistence): incoming payments, written by the
/// engine with the journal rows that pay them; GNAP grants and their access
/// tokens, by hash (spec/decisions/secrets-at-rest.md), written by the
/// authorization server; and which ledger `$host` names.
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

/// Verdicts on Fulfills between peers, as 0.1 kept them
/// (spec/decisions/own-fulfills.md,
/// spec/areas/ledger.md#fulfills-booked-as-final): the Fulfills we sent peers
/// and wait for a verdict on, written with the journal rows that post their
/// packets; the Fulfills we took from peers, kept 30 days to answer a resend
/// the same way; and refusals in the journal, the compensation booked when a
/// peer refused a Fulfill, with the packet's trace.
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

/// Spending through Open Payments (spec/decisions/open-payments.md,
/// spec/areas/payments.md#persistence): quotes, which the resource server
/// writes as it makes them; outgoing payments, written by the engine with the
/// journal rows that move their money; and what each grant spent per window
/// of its interval, written with the payments that spend it.
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
-- connections are closed instead, so senders start anew.
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
-- NULL when only the incoming payment is known
-- (spec/decisions/open-payments.md).
ALTER TABLE quotes ADD COLUMN recipient TEXT
  CHECK (recipient IS NULL OR length(recipient) BETWEEN 1 AND 2048);
ALTER TABLE outgoing_payments ADD COLUMN recipient TEXT
  CHECK (recipient IS NULL OR length(recipient) BETWEEN 1 AND 2048);
";

/// One grace limit per peer, as 0.1 kept it (spec/decisions/own-fulfills.md
/// removed it): the share of the credit we extend that a peer may cost us
/// before it is banned, and why each node was banned. A record outlives its
/// ban, so a peer unbanned is held to account only for what it cost since.
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

/// The XRPL scheme: the fees ledgers charge for the node's
/// transactions are journaled with their transaction, and a scheme keeps
/// what it must across restarts (its transactions in flight, where it is in
/// its ledger, its peers' addresses) with its checkpoint, written in the
/// same batch. The journal is rebuilt, since its CHECKs change.
const MIGRATION_11: &str = "
CREATE TABLE ledger_transfers_new (
  id INTEGER PRIMARY KEY CHECK (id >= 0),
  debit INTEGER NOT NULL REFERENCES ledger_accounts (id),
  credit INTEGER NOT NULL REFERENCES ledger_accounts (id) CHECK (credit != debit),
  amount INTEGER NOT NULL CHECK (amount > 0),
  state TEXT NOT NULL CHECK (state IN ('pending', 'posted', 'voided')),
  created_at_ms INTEGER NOT NULL,
  resolved_at_ms INTEGER CHECK ((state = 'pending') = (resolved_at_ms IS NULL)),
  reference TEXT NOT NULL CHECK (reference IN
    ('packet', 'settlement', 'trust', 'deposit', 'withdrawal', 'reconciliation', 'refusal', 'fee')),
  packet_epoch INTEGER CHECK ((reference IN ('packet', 'refusal')) = (packet_epoch IS NOT NULL)),
  packet_sequence INTEGER CHECK ((reference IN ('packet', 'refusal')) = (packet_sequence IS NOT NULL)),
  settlement_scheme TEXT CHECK ((reference IN ('settlement', 'fee')) = (settlement_scheme IS NOT NULL)),
  settlement_id TEXT CHECK ((reference IN ('settlement', 'fee')) = (settlement_id IS NOT NULL)),
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

ALTER TABLE settlement_checkpoints ADD COLUMN state BLOB
  CHECK (state IS NULL OR length(state) <= 262144);
";

/// Exchange rates, fees and spread (spec/decisions/exchange-rates.md): a
/// Fulfill owed a verdict keeps a third transfer, our fee on transit; the
/// owner sets the fee and the spread, the sources of exchange rates, and the
/// currency totals are shown in; the last rates fetched outlive a restart,
/// each with when it was.
const MIGRATION_12: &str = "
ALTER TABLE owed_fulfills ADD COLUMN debit_3 INTEGER REFERENCES ledger_accounts (id);
ALTER TABLE owed_fulfills ADD COLUMN credit_3 INTEGER REFERENCES ledger_accounts (id);
ALTER TABLE owed_fulfills ADD COLUMN amount_3 INTEGER CHECK (amount_3 IS NULL OR amount_3 > 0);

ALTER TABLE peer_settings ADD COLUMN connector_fee_ppm INTEGER NOT NULL DEFAULT 0
  CHECK (connector_fee_ppm BETWEEN 0 AND 100000);
ALTER TABLE peer_settings ADD COLUMN fx_spread_ppm INTEGER NOT NULL DEFAULT 0
  CHECK (fx_spread_ppm BETWEEN 0 AND 100000);

ALTER TABLE payment_settings ADD COLUMN display_currency TEXT
  CHECK (display_currency IS NULL OR length(display_currency) BETWEEN 1 AND 8);

CREATE TABLE rate_sources (
  position INTEGER PRIMARY KEY CHECK (position BETWEEN 0 AND 7),
  kind TEXT NOT NULL CHECK (kind IN ('coinbase', 'kraken', 'bitstamp')),
  -- Only the Coinbase format is served by others too.
  url TEXT CHECK (url IS NULL OR (kind = 'coinbase' AND length(url) BETWEEN 9 AND 2048))
) STRICT;

INSERT INTO rate_sources VALUES (0, 'coinbase', NULL), (1, 'kraken', NULL), (2, 'bitstamp', NULL);

-- A payment of a fixed amount to send sends all of it; one of a fixed
-- amount to deliver stops once the receiver has it.
ALTER TABLE quotes ADD COLUMN fixed_debit INTEGER NOT NULL DEFAULT 0
  CHECK (fixed_debit IN (0, 1));
ALTER TABLE outgoing_payments ADD COLUMN fixed_debit INTEGER NOT NULL DEFAULT 0
  CHECK (fixed_debit IN (0, 1));

CREATE TABLE exchange_rates (
  asset TEXT PRIMARY KEY CHECK (length(asset) BETWEEN 1 AND 8),
  -- Units of the asset one US dollar buys, times 10^15, in decimal: it may
  -- not fit 64 bits.
  rate TEXT NOT NULL CHECK (length(rate) BETWEEN 1 AND 39
    AND rate NOT GLOB '*[^0-9]*' AND rate NOT GLOB '0*'),
  as_of_ms INTEGER NOT NULL CHECK (as_of_ms >= 0)
) STRICT, WITHOUT ROWID;
";

/// The setup token, kept until setup completes, so that a restart (an
/// update at night, say) does not void the setup URL the owner was given.
const MIGRATION_14: &str = "
CREATE TABLE setup_token (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  token BLOB NOT NULL CHECK (length(token) = 32)
) STRICT;
";

/// Each node books its own Fulfills (spec/decisions/own-fulfills.md): a
/// downstream no longer keeps the Fulfills it owes a verdict, nor an upstream
/// those it took, and no peer is banned for what it cost. What our receiver
/// took for a Fulfill still owed a verdict counts as received, as it would
/// now. The bans the node made for a grace limit are lifted, with every ban's
/// record, and the owner's grace share goes.
const MIGRATION_15: &str = "
UPDATE incoming_payments SET received_amount = received_amount
  + (SELECT sum(o.incoming_amount) FROM owed_fulfills o WHERE o.incoming_payment = incoming_payments.id)
  WHERE id IN (SELECT incoming_payment FROM owed_fulfills WHERE incoming_payment IS NOT NULL);

UPDATE incoming_payments SET completed_at_ms = max(created_at_ms,
    (SELECT max(o.created_at_ms) FROM owed_fulfills o WHERE o.incoming_payment = incoming_payments.id))
  WHERE completed_at_ms IS NULL AND received_amount = incoming_amount
    AND id IN (SELECT incoming_payment FROM owed_fulfills WHERE incoming_payment IS NOT NULL);

INSERT INTO incoming_connections (incoming_payment, connection, received)
  SELECT incoming_payment, incoming_connection, sum(incoming_amount) FROM owed_fulfills
  WHERE incoming_payment IS NOT NULL
    AND incoming_payment IN (SELECT id FROM incoming_payments)
  GROUP BY incoming_payment, incoming_connection
  ON CONFLICT (incoming_payment, connection) DO UPDATE SET received = received + excluded.received;

DROP TABLE owed_fulfills;
DROP TABLE peer_fulfills;

DELETE FROM peer_bans WHERE node IN
  (SELECT node FROM peer_ban_records WHERE reason = 'grace-limit' AND unbanned_at_ms IS NULL);
DROP TABLE peer_ban_records;

ALTER TABLE peer_settings DROP COLUMN grace_share_ppm;
";

/// A node knows only the credit it extends to a peer, not the credit the
/// peer extends to it (spec/decisions/own-fulfills.md).
const MIGRATION_16: &str = "
ALTER TABLE peers DROP COLUMN credit_in;
";

/// Credit grows with the fees a peer earned us
/// (spec/decisions/earned-trust.md): a fee's packet is found by its
/// reference, and where to start reading fees by the last trust transfer.
const MIGRATION_17: &str = "
CREATE INDEX ledger_transfers_by_packet ON ledger_transfers (packet_epoch, packet_sequence)
  WHERE packet_epoch IS NOT NULL;
CREATE INDEX ledger_transfers_trust ON ledger_transfers (sequence) WHERE reference = 'trust';
";

/// How long a peer's room has been short of a packet, settling nothing,
/// counted at checks at which the node could ask
/// (spec/decisions/peering-lifecycle.md): kept, so that a node that restarts
/// often still ends a link that stays short.
const MIGRATION_18: &str = "
ALTER TABLE peers ADD COLUMN short_for_ms INTEGER NOT NULL DEFAULT 0;
";

/// Nothing is settled at a peering's end
/// (spec/decisions/ending-a-peering.md): a final settlement still due when
/// the node upgrades is not made, and what it would have paid stays on the
/// accounts, which a later peering settles as usual.
const MIGRATION_19: &str = "
ALTER TABLE settlement_channels DROP COLUMN final_state;
ALTER TABLE settlement_channels DROP COLUMN final_scheme;
";

/// A peering we are ending, while we collect first and have not told the
/// peer (spec/decisions/ending-a-peering.md): kept, so a restart takes the
/// steps again rather than forgetting the decision.
const MIGRATION_20: &str = "
ALTER TABLE peers ADD COLUMN depeering_since_ms INTEGER;
ALTER TABLE peers ADD COLUMN depeering_reason INTEGER
  CHECK (depeering_reason IS NULL OR depeering_reason BETWEEN 0 AND 255);
";

/// Whether we once ended a peering with the node for want of room: kept
/// with the relationship, so that a later peering with us as the junior
/// raises it no credit (spec/decisions/ended-for-room.md).
const MIGRATION_21: &str = "
ALTER TABLE settlement_channels ADD COLUMN ended_for_room INTEGER NOT NULL DEFAULT 0
  CHECK (ended_for_room IN (0, 1));
";

/// The European Central Bank's reference rates are a source
/// (spec/decisions/exchange-rates.md), so that three sources cover every
/// currency. A node that still has the sources it started with gets it too;
/// an owner who chose others keeps them.
const MIGRATION_22: &str = "
CREATE TABLE rate_sources_new (
  position INTEGER PRIMARY KEY CHECK (position BETWEEN 0 AND 7),
  kind TEXT NOT NULL CHECK (kind IN ('coinbase', 'kraken', 'bitstamp', 'ecb')),
  -- Only the Coinbase format is served by others too.
  url TEXT CHECK (url IS NULL OR (kind = 'coinbase' AND length(url) BETWEEN 9 AND 2048))
) STRICT;

INSERT INTO rate_sources_new SELECT position, kind, url FROM rate_sources;

INSERT INTO rate_sources_new
  SELECT 3, 'ecb', NULL
  WHERE (SELECT count(*) FROM rate_sources) = 3
    AND (SELECT count(*) FROM rate_sources WHERE url IS NULL AND (
      (position = 0 AND kind = 'coinbase') OR
      (position = 1 AND kind = 'kraken') OR
      (position = 2 AND kind = 'bitstamp'))) = 3;

DROP TABLE rate_sources;
ALTER TABLE rate_sources_new RENAME TO rate_sources;
";

/// Strangers never take the owner's share of the grants
/// (spec/decisions/owners-share.md): each grant keeps the network it was
/// asked for from, so no one network holds more than its share. Grants asked
/// for before, and from loopback in development, count in the totals only.
const MIGRATION_23: &str = "
ALTER TABLE gnap_grants ADD COLUMN network BLOB
  CHECK (network IS NULL OR length(network) = 16);
";

/// How much of a peer's time short of room passed while it could be
/// reached (spec/decisions/peering-lifecycle.md): only that ends the peering
/// for room, so a peer our requests to settle never reached is not ended for
/// room it had no chance to restore. Until now, all of it counted, so all of
/// it was reached.
const MIGRATION_24: &str = "
ALTER TABLE peers ADD COLUMN short_reached_ms INTEGER NOT NULL DEFAULT 0;
UPDATE peers SET short_reached_ms = short_for_ms;
";

/// Replay floors move out of the peerings: every sender whose replay state
/// is kept for good has one, since a request from a node registered with
/// us, or from one we hold a channel with, has effects too, and a restart
/// must not take it again. Not tied to `peer_nodes`: a node we hold a
/// channel with may leave the table. Migration 49 drops them.
const MIGRATION_25: &str = "
CREATE TABLE replay_floors (
  node TEXT PRIMARY KEY CHECK (length(node) BETWEEN 2 AND 45),
  floor_ms INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

INSERT INTO replay_floors (node, floor_ms) SELECT node, replay_floor_ms FROM peers;

ALTER TABLE peers DROP COLUMN replay_floor_ms;
";

/// External assets (spec/areas/settlement.md#external-assets): what the
/// node holds outside its books keeps its own state, apart from the schemes
/// that settle through it. The `xrpl-testnet` scheme's state is split
/// between it and the node's first XRPL testnet account: the account takes
/// the sequence, the transactions in flight (each the scheme's) and the
/// newest ledger seen; the scheme keeps its cursor, its peers' addresses and
/// the settlements it looks up. Nothing is booked anew
/// (spec/areas/xrpl.md#state-and-restarts). A state is a BLOB, which the
/// JSON functions would read as JSONB, so it is read as text.
const MIGRATION_26: &str = "
CREATE TABLE asset_checkpoints (
  asset TEXT PRIMARY KEY CHECK (length(asset) BETWEEN 1 AND 64),
  updated_at_ms INTEGER NOT NULL,
  state BLOB NOT NULL CHECK (length(state) <= 262144)
) STRICT, WITHOUT ROWID;

INSERT INTO asset_checkpoints (asset, updated_at_ms, state)
  SELECT 'xrpl-testnet-account/0', updated_at_ms, CAST(json_object(
    'sequence', json_extract(CAST(state AS TEXT), '$.sequence'),
    'in_flight', json((
      SELECT json_group_array(json_set(value, '$.scheme', 'xrpl-testnet'))
      FROM json_each(CAST(state AS TEXT), '$.in_flight'))),
    'never_sent', json_array(),
    'newest', json_extract(CAST(state AS TEXT), '$.newest')
  ) AS BLOB)
  FROM settlement_checkpoints WHERE scheme = 'xrpl-testnet' AND state IS NOT NULL;

UPDATE settlement_checkpoints
  SET state = CAST(json_remove(CAST(state AS TEXT), '$.sequence', '$.in_flight', '$.newest') AS BLOB)
  WHERE scheme = 'xrpl-testnet' AND state IS NOT NULL;
";

/// Paired devices (spec/areas/rpc.md#paired-devices): owner apps, each
/// named by the node ID of its device key, which proves itself on every
/// connection. The key is public, so nothing here is a secret.
const MIGRATION_27: &str = "
CREATE TABLE devices (
  id TEXT PRIMARY KEY CHECK (length(id) BETWEEN 2 AND 45),
  name TEXT NOT NULL CHECK (length(CAST(name AS BLOB)) BETWEEN 1 AND 64),
  paired_at_ms INTEGER NOT NULL,
  last_seen_at_ms INTEGER NOT NULL CHECK (last_seen_at_ms >= paired_at_ms)
) STRICT, WITHOUT ROWID;
";

/// An incoming payment counts as paid in full within a tolerance, over or
/// under its amount (spec/decisions/paid-within-a-tolerance.md), so what it
/// received may exceed its amount, by at most the tolerance, which the
/// engine checks. SQLite changes a check only by building the table anew;
/// its connections are set aside meanwhile, since dropping the payments
/// would delete them.
const MIGRATION_28: &str = "
CREATE TABLE incoming_connections_kept AS SELECT * FROM incoming_connections;
DROP TABLE incoming_connections;

CREATE TABLE incoming_payments_new (
  id BLOB PRIMARY KEY CHECK (length(id) = 16),
  ledger TEXT NOT NULL,
  wallet_address TEXT NOT NULL CHECK (length(wallet_address) BETWEEN 1 AND 2048),
  -- The client that created it, as grants name clients; NULL for the owner.
  client TEXT CHECK (client IS NULL OR length(client) BETWEEN 1 AND 2048),
  incoming_amount INTEGER CHECK (incoming_amount IS NULL OR incoming_amount > 0),
  -- At most the tolerance over its amount, which the engine checks.
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

INSERT INTO incoming_payments_new SELECT * FROM incoming_payments;
DROP TABLE incoming_payments;
ALTER TABLE incoming_payments_new RENAME TO incoming_payments;

CREATE INDEX incoming_payments_by_ledger ON incoming_payments (ledger, created_at_ms, id);
CREATE INDEX incoming_payments_by_client ON incoming_payments (ledger, client, created_at_ms, id);
CREATE INDEX incoming_payments_by_time ON incoming_payments (created_at_ms, id);
CREATE INDEX incoming_payments_open ON incoming_payments (expires_at_ms) WHERE completed_at_ms IS NULL;
CREATE INDEX incoming_payments_unpaid ON incoming_payments (coalesce(completed_at_ms, expires_at_ms))
  WHERE received_amount = 0;

CREATE TABLE incoming_connections (
  incoming_payment BLOB NOT NULL REFERENCES incoming_payments (id) ON DELETE CASCADE,
  -- Which credentials of the payment the connection uses; zeros for those
  -- issued with the payment before connections had keys.
  connection BLOB NOT NULL CHECK (length(connection) = 16),
  received INTEGER NOT NULL CHECK (received > 0),
  PRIMARY KEY (incoming_payment, connection)
) STRICT, WITHOUT ROWID;

INSERT INTO incoming_connections SELECT * FROM incoming_connections_kept;
DROP TABLE incoming_connections_kept;
";

/// Conversions between peers are priced by the owner's inventory
/// (spec/decisions/passive-rebalancing.md): the owner sets the curve's `k`,
/// which every node starts with, an upgraded one too.
const MIGRATION_29: &str = "
ALTER TABLE peer_settings ADD COLUMN rebalance_ppm INTEGER NOT NULL DEFAULT 50000
  CHECK (rebalance_ppm BETWEEN 0 AND 100000);
";

/// Settlement hubs (spec/areas/hubs.md#the-members-side): what a peering
/// over a hub cost beyond its hold is journaled as a loss, keyed by its
/// scheme and the hub's ID for it. The journal is rebuilt, since its
/// CHECKs change.
const MIGRATION_30: &str = "
CREATE TABLE ledger_transfers_new (
  id INTEGER PRIMARY KEY CHECK (id >= 0),
  debit INTEGER NOT NULL REFERENCES ledger_accounts (id),
  credit INTEGER NOT NULL REFERENCES ledger_accounts (id) CHECK (credit != debit),
  amount INTEGER NOT NULL CHECK (amount > 0),
  state TEXT NOT NULL CHECK (state IN ('pending', 'posted', 'voided')),
  created_at_ms INTEGER NOT NULL,
  resolved_at_ms INTEGER CHECK ((state = 'pending') = (resolved_at_ms IS NULL)),
  reference TEXT NOT NULL CHECK (reference IN
    ('packet', 'settlement', 'trust', 'deposit', 'withdrawal', 'reconciliation', 'refusal', 'fee',
     'loss')),
  packet_epoch INTEGER CHECK ((reference IN ('packet', 'refusal')) = (packet_epoch IS NOT NULL)),
  packet_sequence INTEGER CHECK ((reference IN ('packet', 'refusal')) = (packet_sequence IS NOT NULL)),
  settlement_scheme TEXT
    CHECK ((reference IN ('settlement', 'fee', 'loss')) = (settlement_scheme IS NOT NULL)),
  settlement_id TEXT
    CHECK ((reference IN ('settlement', 'fee', 'loss')) = (settlement_id IS NOT NULL)),
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
CREATE INDEX ledger_transfers_by_packet ON ledger_transfers (packet_epoch, packet_sequence)
  WHERE packet_epoch IS NOT NULL;
CREATE INDEX ledger_transfers_trust ON ledger_transfers (sequence) WHERE reference = 'trust';
";

/// A settlement channel left idle closes: its row stays, so that its
/// sequences and whether we ended a peering for room outlive it, and it
/// leaves memory (spec/decisions/closing-channels.md). Only a channel with
/// nothing on its way closes.
const MIGRATION_31: &str = "
ALTER TABLE settlement_channels ADD COLUMN closed INTEGER NOT NULL DEFAULT 0
  CHECK (closed IN (0, 1) AND (closed = 0 OR outbox_sequence IS NULL));
";

/// The settlement hubs the node validates (spec/areas/hubs.md#the-validator-in-the-node),
/// each by its ID: its consensus state, and the states after its recent
/// final blocks and after the last blocks of recent epochs, the newest
/// written in a step's transaction before its messages go out, the others
/// kept to serve validators far behind; how far it followed the escrow on
/// the XRP Ledger, and the newest ledger it saw there, to tell a reset from
/// a server behind (spec/decisions/ledger-resets.md); and the payout
/// bundles it signed. The states are the hub crate's encodings
/// (`store/hubs.rs` has the bounds).
const MIGRATION_32: &str = "
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
";

/// Founding settlement hubs (spec/areas/hubs.md#founding-a-hub): what a
/// validator keeps of each founding it took part in, by the terms' nonce,
/// the genesis it signed and whether it runs the hub; a sponsor's foundings
/// and their escrow's funding; and the payout bundles a member read from
/// its hub's validators, the newest of each.
const MIGRATION_34: &str = "
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
  updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0)
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

/// Founding settlement hubs (spec/areas/hubs.md#founding-a-hub): why a
/// sponsor stopped funding a founding's escrow for good, so that a restart
/// never pays again what failed: too little to create the account, or a
/// failure of another kind the ledger recorded.
const MIGRATION_35: &str = "
ALTER TABLE hub_foundings ADD COLUMN escrow_stop TEXT
  CHECK (escrow_stop IN ('too-little', 'failed'));
";

/// Settlement hubs (spec/areas/hubs.md#in-the-node): whether the node moves
/// funds between its XRPL account and its hub by itself, which the owner
/// may turn off; until when an owner's move pauses that; and the move on
/// its way, with the counter value that shows it done and its payment, so
/// that a restart never pays it twice; and the move the owner gave up,
/// while it holds the node's own moves back. The journal is rebuilt, as its
/// CHECKs change: the owner's payment on its way is a pending transfer of
/// its own kind, which outlives a restart.
const MIGRATION_33: &str = "
CREATE TABLE hub_funding (
  id INTEGER PRIMARY KEY CHECK (id = 1),
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
) STRICT;
INSERT INTO hub_funding (id, automatic, paused_until_ms) VALUES (1, 1, 0);

CREATE TABLE ledger_transfers_new (
  id INTEGER PRIMARY KEY CHECK (id >= 0),
  debit INTEGER NOT NULL REFERENCES ledger_accounts (id),
  credit INTEGER NOT NULL REFERENCES ledger_accounts (id) CHECK (credit != debit),
  amount INTEGER NOT NULL CHECK (amount > 0),
  state TEXT NOT NULL CHECK (state IN ('pending', 'posted', 'voided')),
  created_at_ms INTEGER NOT NULL,
  resolved_at_ms INTEGER CHECK ((state = 'pending') = (resolved_at_ms IS NULL)),
  reference TEXT NOT NULL CHECK (reference IN
    ('packet', 'settlement', 'trust', 'deposit', 'withdrawal', 'reconciliation', 'refusal', 'fee',
     'loss', 'owner_payment')),
  packet_epoch INTEGER CHECK ((reference IN ('packet', 'refusal')) = (packet_epoch IS NOT NULL)),
  packet_sequence INTEGER CHECK ((reference IN ('packet', 'refusal')) = (packet_sequence IS NOT NULL)),
  settlement_scheme TEXT
    CHECK ((reference IN ('settlement', 'fee', 'loss', 'owner_payment'))
      = (settlement_scheme IS NOT NULL)),
  settlement_id TEXT
    CHECK ((reference IN ('settlement', 'fee', 'loss', 'owner_payment'))
      = (settlement_id IS NOT NULL)),
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
CREATE INDEX ledger_transfers_by_packet ON ledger_transfers (packet_epoch, packet_sequence)
  WHERE packet_epoch IS NOT NULL;
CREATE INDEX ledger_transfers_trust ON ledger_transfers (sequence) WHERE reference = 'trust';
";

/// A grant that creates outgoing payments under a receive limit alone
/// bounds nothing the owner pays, so such grants are refused when asked
/// for, and those kept from before, pending or approved, are revoked
/// (spec/areas/payments.md#what-a-grant-allows): their tokens go, as when
/// the owner revokes one, and each gets a revision of its own. A pending
/// one counts as decided when it was asked for, since a migration has no
/// clock; nothing reads when a revoked grant was decided. Items that only
/// read keep their limits, which bound no payment.
const MIGRATION_36: &str = "
CREATE TEMP TABLE receive_alone_grants AS
  SELECT g.id,
    (SELECT coalesce(max(revision), 0) FROM gnap_grants) + row_number() OVER (ORDER BY g.id)
      AS revision
  FROM gnap_grants g
  WHERE g.state IN ('pending', 'approved') AND EXISTS (
    SELECT 1 FROM json_each(g.access) item
    WHERE json_extract(item.value, '$.type') = 'outgoing-payment'
      AND EXISTS (SELECT 1 FROM json_each(item.value, '$.actions') action
        WHERE action.value = 'create')
      AND json_extract(item.value, '$.limits.receiveAmount') IS NOT NULL
      AND json_extract(item.value, '$.limits.debitAmount') IS NULL);

DELETE FROM gnap_tokens WHERE grant_id IN (SELECT id FROM temp.receive_alone_grants);

UPDATE gnap_grants SET state = 'revoked',
  decided_at_ms = coalesce(decided_at_ms, created_at_ms),
  revision = (SELECT r.revision FROM temp.receive_alone_grants r WHERE r.id = gnap_grants.id)
WHERE id IN (SELECT id FROM temp.receive_alone_grants);

DROP TABLE temp.receive_alone_grants;
";

/// A registrar's log and names (spec/areas/network.md#names): where the log
/// stands, which only rises, the commitments in it with their unsealings,
/// and the names granted, held or freed a short while ago. Every row is
/// written before the answer that it backs is sent, so a restart loses no
/// receipted commitment and reorders nothing.
const MIGRATION_37: &str = "
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
";

/// Who holds each name, in the node's view (spec/areas/network.md#holders):
/// the holder a quorum of the registrars' lists name, kept while the view
/// is frozen, across restarts too.
const MIGRATION_38: &str = "
CREATE TABLE name_holders (
  name TEXT PRIMARY KEY CHECK (length(name) BETWEEN 3 AND 32),
  holder TEXT NOT NULL CHECK (length(holder) BETWEEN 2 AND 45),
  expires_ms INTEGER NOT NULL
) STRICT, WITHOUT ROWID;
";

/// The node's registrations of names (spec/areas/network.md#the-registrant):
/// each with the salt its commitment is sealed with, written before its
/// first commitment leaves, so a restart commits to the same commitment
/// again.
const MIGRATION_39: &str = "
CREATE TABLE name_registrations (
  name TEXT PRIMARY KEY CHECK (length(name) BETWEEN 3 AND 32),
  salt BLOB NOT NULL CHECK (length(salt) = 32),
  started_ms INTEGER NOT NULL,
  stage TEXT NOT NULL CHECK (stage IN ('committing', 'unsealing', 'abandoned'))
) STRICT, WITHOUT ROWID;
";

/// Whether the node ever paid us a settlement of at least one largest
/// packet: kept with the relationship, open or closed, so that a downlink
/// that paid us is no free receiver when it comes back
/// (spec/decisions/free-receivers-half.md).
const MIGRATION_40: &str = "
ALTER TABLE settlement_channels ADD COLUMN paid_us INTEGER NOT NULL DEFAULT 0
  CHECK (paid_us IN (0, 1));
";

/// The peers' accounts opened for downlinks, by ledger and node: they take
/// the downlinks' share of the peers' account room, for good, as the
/// accounts stay (spec/decisions/account-budgets.md). A node that kept no
/// record has its downlinks' accounts found by their settlement channels,
/// whose URL is empty for a downlink.
const MIGRATION_41: &str = "
CREATE TABLE downlink_accounts (
  ledger TEXT NOT NULL CHECK (length(ledger) BETWEEN 1 AND 32),
  node TEXT NOT NULL CHECK (length(node) BETWEEN 2 AND 45),
  PRIMARY KEY (ledger, node)
) STRICT, WITHOUT ROWID;
INSERT INTO downlink_accounts (ledger, node)
  SELECT substr(a.path, 1, instr(a.path, ':') - 1), c.node
  FROM settlement_channels c JOIN ledger_accounts a
    ON substr(a.path, instr(a.path, ':') + 1) = 'assets/interledger/' || c.node
  WHERE c.url = '';
";

/// The testnet hub's scheme takes a name that says its ledger and its
/// asset, `hub-xrpl-testnet-xrp`, and its ledger with it,
/// `hub-xrpl-testnet-xrp+xrp` (spec/areas/hubs.md#the-members-side).
/// Everything stored under the old names moves: the scheme, its
/// checkpoint, the journal's references to it, the peerings and the
/// settlements on their way over it, and the ledger's accounts, so the
/// books keep their balances. The ledger's slug names its wallet address,
/// so the payments and grants of the old wallet address follow it to the
/// new one, and so does the subject a grant asked about. A grant whose
/// access or interaction would grow past its bound keeps the old address
/// there, and so no longer reaches the ledger. The payments and grants
/// that change take revisions above every other, as when they are
/// written, so live views see the change. The receiver's address and
/// secret move too, so credentials it issued before no longer pay, and so
/// does the payer's address (spec/areas/ledger.md#ledgers-and-amounts).
const MIGRATION_42: &str = "
UPDATE settlement_schemes SET id = 'hub-xrpl-testnet-xrp' WHERE id = 'hub-xrp-testnet';
UPDATE settlement_checkpoints SET scheme = 'hub-xrpl-testnet-xrp' WHERE scheme = 'hub-xrp-testnet';
UPDATE ledger_transfers SET settlement_scheme = 'hub-xrpl-testnet-xrp'
  WHERE settlement_scheme = 'hub-xrp-testnet';
UPDATE peers SET scheme = 'hub-xrpl-testnet-xrp' WHERE scheme = 'hub-xrp-testnet';
UPDATE settlement_channels SET outbox_scheme = 'hub-xrpl-testnet-xrp'
  WHERE outbox_scheme = 'hub-xrp-testnet';

UPDATE ledger_accounts
  SET path = 'hub-xrpl-testnet-xrp+xrp:' || substr(path, length('hub-xrp-testnet+xrp:') + 1)
  WHERE substr(path, 1, length('hub-xrp-testnet+xrp:')) = 'hub-xrp-testnet+xrp:';
UPDATE downlink_accounts SET ledger = 'hub-xrpl-testnet-xrp+xrp' WHERE ledger = 'hub-xrp-testnet+xrp';
UPDATE btp_tokens SET ledger = 'hub-xrpl-testnet-xrp+xrp' WHERE ledger = 'hub-xrp-testnet+xrp';
UPDATE payment_settings SET default_ledger = 'hub-xrpl-testnet-xrp+xrp'
  WHERE default_ledger = 'hub-xrp-testnet+xrp';

CREATE TEMP TABLE renamed_incoming AS
  SELECT id,
    (SELECT coalesce(max(revision), 0) FROM incoming_payments) + row_number() OVER (ORDER BY revision)
      AS revision
  FROM incoming_payments
  WHERE ledger = 'hub-xrp-testnet+xrp'
    OR substr(wallet_address, -length('/pay/hub-xrp-testnet')) = '/pay/hub-xrp-testnet';
UPDATE incoming_payments
  SET revision = (SELECT r.revision FROM temp.renamed_incoming r WHERE r.id = incoming_payments.id)
  WHERE id IN (SELECT id FROM temp.renamed_incoming);
DROP TABLE temp.renamed_incoming;

CREATE TEMP TABLE renamed_outgoing AS
  SELECT id,
    (SELECT coalesce(max(revision), 0) FROM outgoing_payments) + row_number() OVER (ORDER BY revision)
      AS revision
  FROM outgoing_payments
  WHERE ledger = 'hub-xrp-testnet+xrp'
    OR substr(wallet_address, -length('/pay/hub-xrp-testnet')) = '/pay/hub-xrp-testnet';
UPDATE outgoing_payments
  SET revision = (SELECT r.revision FROM temp.renamed_outgoing r WHERE r.id = outgoing_payments.id)
  WHERE id IN (SELECT id FROM temp.renamed_outgoing);
DROP TABLE temp.renamed_outgoing;

UPDATE incoming_payments SET ledger = 'hub-xrpl-testnet-xrp+xrp' WHERE ledger = 'hub-xrp-testnet+xrp';
UPDATE quotes SET ledger = 'hub-xrpl-testnet-xrp+xrp' WHERE ledger = 'hub-xrp-testnet+xrp';
UPDATE outgoing_payments SET ledger = 'hub-xrpl-testnet-xrp+xrp' WHERE ledger = 'hub-xrp-testnet+xrp';
UPDATE incoming_payments
  SET wallet_address = substr(wallet_address, 1, length(wallet_address) - length('hub-xrp-testnet'))
    || 'hub-xrpl-testnet-xrp'
  WHERE substr(wallet_address, -length('/pay/hub-xrp-testnet')) = '/pay/hub-xrp-testnet';
UPDATE quotes
  SET wallet_address = substr(wallet_address, 1, length(wallet_address) - length('hub-xrp-testnet'))
    || 'hub-xrpl-testnet-xrp'
  WHERE substr(wallet_address, -length('/pay/hub-xrp-testnet')) = '/pay/hub-xrp-testnet';
UPDATE outgoing_payments
  SET wallet_address = substr(wallet_address, 1, length(wallet_address) - length('hub-xrp-testnet'))
    || 'hub-xrpl-testnet-xrp'
  WHERE substr(wallet_address, -length('/pay/hub-xrp-testnet')) = '/pay/hub-xrp-testnet';

-- A grant's access names wallet addresses as identifiers, and its
-- interaction the one subject it asked about (`sub_ids` holds exactly one).
CREATE TEMP TABLE renamed_grants AS
  SELECT id, access, interaction, in_access, in_subject,
    (SELECT coalesce(max(revision), 0) FROM gnap_grants) + row_number() OVER (ORDER BY revision)
      AS revision
  FROM (
    SELECT id, revision, new_access AS access, new_interaction AS interaction,
      instr(access, '/pay/hub-xrp-testnet\"') > 0 AND length(new_access) <= 16384 AS in_access,
      coalesce(substr(subject, -length('/pay/hub-xrp-testnet')) = '/pay/hub-xrp-testnet'
        AND length(new_interaction) <= 4096, 0) AS in_subject
    FROM (
      SELECT id, revision, access, subject,
        replace(access, '/pay/hub-xrp-testnet\"', '/pay/hub-xrpl-testnet-xrp\"') AS new_access,
        json_set(interaction, '$.subject.sub_ids[0].id',
          substr(subject, 1, length(subject) - length('hub-xrp-testnet')) || 'hub-xrpl-testnet-xrp')
          AS new_interaction
      FROM (
        SELECT id, revision, access, interaction,
          json_extract(interaction, '$.subject.sub_ids[0].id') AS subject
        FROM gnap_grants)))
  WHERE in_access OR in_subject;
UPDATE gnap_grants
  SET revision = (SELECT r.revision FROM temp.renamed_grants r WHERE r.id = gnap_grants.id)
  WHERE id IN (SELECT id FROM temp.renamed_grants);
UPDATE gnap_grants
  SET access = (SELECT r.access FROM temp.renamed_grants r WHERE r.id = gnap_grants.id)
  WHERE id IN (SELECT id FROM temp.renamed_grants WHERE in_access);
UPDATE gnap_grants
  SET interaction = (SELECT r.interaction FROM temp.renamed_grants r WHERE r.id = gnap_grants.id)
  WHERE id IN (SELECT id FROM temp.renamed_grants WHERE in_subject);
DROP TABLE temp.renamed_grants;
";

/// No node books a `refusal` any more: each keeps its own Fulfills
/// (spec/decisions/own-fulfills.md). Rows of that kind a journal still holds
/// become `reconciliation` rows, which name no packet, with their accounts
/// and amounts as they were, so the books stay what they were. The journal
/// is rebuilt, since its CHECKs change, and the index on refusals goes.
const MIGRATION_43: &str = "
CREATE TABLE ledger_transfers_new (
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
  sequence INTEGER NOT NULL UNIQUE CHECK (sequence > 0)
) STRICT;

INSERT INTO ledger_transfers_new
  SELECT id, debit, credit, amount, state, created_at_ms, resolved_at_ms,
    CASE reference WHEN 'refusal' THEN 'reconciliation' ELSE reference END,
    CASE reference WHEN 'refusal' THEN NULL ELSE packet_epoch END,
    CASE reference WHEN 'refusal' THEN NULL ELSE packet_sequence END,
    settlement_scheme, settlement_id, sequence
  FROM ledger_transfers;
DROP TABLE ledger_transfers;
ALTER TABLE ledger_transfers_new RENAME TO ledger_transfers;

CREATE INDEX ledger_transfers_by_debit ON ledger_transfers (debit, state);
CREATE INDEX ledger_transfers_by_credit ON ledger_transfers (credit, state);
CREATE INDEX ledger_transfers_pending ON ledger_transfers (state) WHERE state = 'pending';
CREATE INDEX ledger_transfers_by_debit_sequence ON ledger_transfers (debit, sequence);
CREATE INDEX ledger_transfers_by_credit_sequence ON ledger_transfers (credit, sequence);
CREATE INDEX ledger_transfers_by_packet ON ledger_transfers (packet_epoch, packet_sequence)
  WHERE packet_epoch IS NOT NULL;
CREATE INDEX ledger_transfers_trust ON ledger_transfers (sequence) WHERE reference = 'trust';
";

/// The XRP Ledger accounts and their schemes keep their state in a strict,
/// versioned encoding (spec/areas/xrpl.md#state-and-restarts), which reads
/// none of the JSON written before. What they kept is dropped, and the
/// node reads each account anew from the ledger at the next start, as a
/// first start does, the schemes' balances kept: what the ledger holds
/// beyond the books, or short of them, is reconciled through suspense, a
/// settlement the books hold pending is looked up by its hash, and the
/// lists are fetched again. So a transaction signed and not yet sent is
/// looked up as any unknown one, and stays pending until found; a peer's
/// address is learnt again when the node next settles with it, and a
/// settlement from it before then is booked as a deposit; and the entry
/// payments held are in the balance, reconciled through suspense.
const MIGRATION_44: &str = "
DELETE FROM asset_checkpoints WHERE asset LIKE 'xrpl-%';
UPDATE settlement_checkpoints SET state = NULL WHERE scheme LIKE 'xrpl%';
";

/// An external asset's config is its own, set by the owner apart from any
/// scheme's (spec/areas/settlement.md#external-assets): for an XRPL
/// account, its server, its faucet and its validator list publishers,
/// which every scheme on it had to name alike. Each account takes what the
/// first of its schemes, by ID, said of it, and the schemes keep only
/// whether they peer. A disabled scheme (test RLUSD) gives nothing: the node
/// never read its config, so its server must not become the account's.
const MIGRATION_45: &str = "
CREATE TABLE asset_configs (
  asset TEXT PRIMARY KEY CHECK (length(asset) BETWEEN 1 AND 64),
  config TEXT NOT NULL CHECK (length(CAST(config AS BLOB)) BETWEEN 1 AND 4096),
  updated_at_ms INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

INSERT INTO asset_configs (asset, config, updated_at_ms)
  SELECT
    CASE WHEN id IN ('xrpl', 'xrpl-rlusd') THEN 'xrpl-account/0' ELSE 'xrpl-testnet-account/0' END,
    json_remove(config, '$.peers'),
    activated_at_ms
  FROM settlement_schemes
  WHERE id IN ('xrpl-testnet', 'xrpl-testnet-dsusd', 'xrpl', 'xrpl-rlusd')
    AND json_valid(config) AND json_type(config) = 'object'
    AND json_remove(config, '$.peers') <> '{}'
  ORDER BY id
  ON CONFLICT (asset) DO NOTHING;

UPDATE settlement_schemes
  SET config = CASE
    WHEN json_valid(config) AND json_type(config, '$.peers') = 'false' THEN '{\"peers\":false}'
    ELSE ''
  END
  WHERE id IN ('xrpl-testnet', 'xrpl-testnet-dsusd', 'xrpl-testnet-rlusd', 'xrpl', 'xrpl-rlusd');
";

/// Renames settlement scheme `$old` to `$new`, and its ledger `$old_ledger`
/// to `$new_ledger`, everywhere the books keep them, as migration 42 renamed
/// the hub's: the scheme, its checkpoint, the journal's references to it,
/// the peerings and the settlements on their way over it, the ledger's
/// accounts, so the books keep their balances, its downlinks, BTP tokens,
/// the default ledger, and the payments and quotes on it, which take new
/// revisions above every other, so live views see them.
macro_rules! rename_scheme {
    ($old:literal, $new:literal, $old_ledger:literal, $new_ledger:literal) => {
        concat!(
            "UPDATE settlement_schemes SET id = '",
            $new,
            "' WHERE id = '",
            $old,
            "';
UPDATE settlement_checkpoints SET scheme = '",
            $new,
            "' WHERE scheme = '",
            $old,
            "';
UPDATE ledger_transfers SET settlement_scheme = '",
            $new,
            "' WHERE settlement_scheme = '",
            $old,
            "';
UPDATE peers SET scheme = '",
            $new,
            "' WHERE scheme = '",
            $old,
            "';
UPDATE settlement_channels SET outbox_scheme = '",
            $new,
            "' WHERE outbox_scheme = '",
            $old,
            "';
UPDATE ledger_accounts
  SET path = '",
            $new_ledger,
            ":' || substr(path, length('",
            $old_ledger,
            ":') + 1)
  WHERE substr(path, 1, length('",
            $old_ledger,
            ":')) = '",
            $old_ledger,
            ":';
UPDATE downlink_accounts SET ledger = '",
            $new_ledger,
            "' WHERE ledger = '",
            $old_ledger,
            "';
UPDATE btp_tokens SET ledger = '",
            $new_ledger,
            "' WHERE ledger = '",
            $old_ledger,
            "';
UPDATE payment_settings SET default_ledger = '",
            $new_ledger,
            "'
  WHERE default_ledger = '",
            $old_ledger,
            "';
UPDATE incoming_payments
  SET ledger = '",
            $new_ledger,
            "',
    revision = (SELECT coalesce(max(revision), 0) FROM incoming_payments) + revision
  WHERE ledger = '",
            $old_ledger,
            "';
UPDATE outgoing_payments
  SET ledger = '",
            $new_ledger,
            "',
    revision = (SELECT coalesce(max(revision), 0) FROM outgoing_payments) + revision
  WHERE ledger = '",
            $old_ledger,
            "';
UPDATE quotes SET ledger = '",
            $new_ledger,
            "' WHERE ledger = '",
            $old_ledger,
            "';
"
        )
    };
}

/// Moves wallet address `/pay/$old` to `/pay/$new` wherever the books keep
/// it, as migration 42 moved the hub's: in incoming and outgoing payments
/// and quotes, and in grants, as an access item's identifier and as the
/// subject an interaction asked about (`sub_ids` holds exactly one). A
/// grant whose access or interaction would grow past its column's bound
/// keeps the old address there. What changes takes new revisions above
/// every other, so live views see it.
macro_rules! rename_wallet {
    ($old:literal, $new:literal) => {
        concat!(
            "UPDATE incoming_payments
  SET wallet_address = substr(wallet_address, 1, length(wallet_address) - length('", $old, "'))
      || '", $new, "',
    revision = (SELECT coalesce(max(revision), 0) FROM incoming_payments) + revision
  WHERE substr(wallet_address, -length('/pay/", $old, "')) = '/pay/", $old, "';
UPDATE outgoing_payments
  SET wallet_address = substr(wallet_address, 1, length(wallet_address) - length('", $old, "'))
      || '", $new, "',
    revision = (SELECT coalesce(max(revision), 0) FROM outgoing_payments) + revision
  WHERE substr(wallet_address, -length('/pay/", $old, "')) = '/pay/", $old, "';
UPDATE quotes
  SET wallet_address = substr(wallet_address, 1, length(wallet_address) - length('", $old, "'))
    || '", $new, "'
  WHERE substr(wallet_address, -length('/pay/", $old, "')) = '/pay/", $old, "';
CREATE TEMP TABLE renamed_grants AS
  SELECT id, access, interaction, in_access, in_subject
  FROM (
    SELECT id, new_access AS access, new_interaction AS interaction,
      instr(access, '/pay/", $old, "\"') > 0 AND length(new_access) <= 16384 AS in_access,
      coalesce(substr(subject, -length('/pay/", $old, "')) = '/pay/", $old, "'
        AND length(new_interaction) <= 4096, 0) AS in_subject
    FROM (
      SELECT id, access, subject,
        replace(access, '/pay/", $old, "\"', '/pay/", $new, "\"') AS new_access,
        json_set(interaction, '$.subject.sub_ids[0].id',
          substr(subject, 1, length(subject) - length('", $old, "')) || '", $new, "')
          AS new_interaction
      FROM (
        SELECT id, access, interaction,
          json_extract(interaction, '$.subject.sub_ids[0].id') AS subject
        FROM gnap_grants)))
  WHERE in_access OR in_subject;
UPDATE gnap_grants
  SET access = (SELECT r.access FROM temp.renamed_grants r WHERE r.id = gnap_grants.id)
  WHERE id IN (SELECT id FROM temp.renamed_grants WHERE in_access);
UPDATE gnap_grants
  SET interaction = (SELECT r.interaction FROM temp.renamed_grants r WHERE r.id = gnap_grants.id)
  WHERE id IN (SELECT id FROM temp.renamed_grants WHERE in_subject);
UPDATE gnap_grants
  SET revision = (SELECT coalesce(max(revision), 0) FROM gnap_grants) + revision
  WHERE id IN (SELECT id FROM temp.renamed_grants);
DROP TABLE temp.renamed_grants;
"
        )
    };
}

/// Every scheme's ID names its ledger, its network but mainnet, and its
/// asset, so the XRP schemes take theirs, `xrpl-testnet-xrp` and
/// `xrpl-xrp`, with their ledgers (spec/decisions/scheme-names.md). A
/// ledger's wallet address names it for people, by its asset, apart from
/// its slug: `/pay/usd`, `/pay/xrp`, `/pay/dsusd`, `/pay/rlusd`,
/// `/pay/hub-xrp`. The receiver's and the payer's addresses follow the
/// renamed ledgers' slugs, so credentials their receivers issued before
/// no longer pay, as after migration 42.
const MIGRATION_46: &str = concat!(
    rename_scheme!(
        "xrpl-testnet",
        "xrpl-testnet-xrp",
        "xrpl-testnet+xrp",
        "xrpl-testnet-xrp+xrp"
    ),
    rename_scheme!("xrpl", "xrpl-xrp", "xrpl+xrp", "xrpl-xrp+xrp"),
    rename_wallet!("stub", "usd"),
    rename_wallet!("xrpl-testnet", "xrp"),
    rename_wallet!("xrpl", "xrp"),
    rename_wallet!("xrpl-testnet-rlusd", "rlusd"),
    rename_wallet!("xrpl-rlusd", "rlusd"),
    rename_wallet!("xrpl-testnet-dsusd", "dsusd"),
    rename_wallet!("hub-xrpl-testnet-xrp", "hub-xrp"),
);

/// The standalone runtime binds 80 and 443 unless its flags say otherwise
/// (spec/areas/node.md#runtimes): the owner no longer sets ports, so the
/// stored ones go. `https_port` goes first, since its CHECK names
/// `http_port`.
const MIGRATION_47: &str = "
ALTER TABLE node_config DROP COLUMN https_port;
ALTER TABLE node_config DROP COLUMN http_port;
";

/// A peering names no scheme version: a scheme's meaning is versioned by
/// its ID and by the peer protocol's version
/// (spec/areas/peer-protocol.md#versions).
const MIGRATION_48: &str = "
ALTER TABLE peers DROP COLUMN version;
";

/// A request is the request of the node whose key its QUIC connection
/// proved, so nothing is kept against replays
/// (spec/areas/peer-protocol.md#authentication).
const MIGRATION_49: &str = "
DROP TABLE replay_floors;
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
            ("scheme", "TEXT", true, false),
            ("state", "BLOB", true, false),
            ("credit_out", "INTEGER", true, false),
            ("since_ms", "INTEGER", true, false),
            ("inbound", "INTEGER", true, false),
            ("last_seen_ms", "INTEGER", true, false),
            ("short_for_ms", "INTEGER", true, false),
            ("depeering_since_ms", "INTEGER", false, false),
            ("depeering_reason", "INTEGER", false, false),
            ("short_reached_ms", "INTEGER", true, false),
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
            ("ended_for_room", "INTEGER", true, false),
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
            ("default_ledger", "TEXT", false, false),
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
        without_rowid: false,
        columns: &[
            ("id", "INTEGER", false, true),
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

/// The schema version of a Dassie database that this code will migrate,
/// if it is one: neither new, nor current, nor too new, nor foreign.
pub fn pending_migration(connection: &Connection) -> Result<Option<i64>, StoreError> {
    let application_id: i32 =
        connection.pragma_query_value(None, "application_id", |row| row.get(0))?;
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    Ok(
        (application_id == APPLICATION_ID && version > 0 && version < SCHEMA_VERSION)
            .then_some(version),
    )
}

fn migrate(connection: &mut Connection) -> Result<(), StoreError> {
    migrate_until(connection, MIGRATIONS.len())
}

/// Runs the migrations up to `last`, for tests of an upgrade.
pub(super) fn migrate_until(connection: &mut Connection, last: usize) -> Result<(), StoreError> {
    let current: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if current > SCHEMA_VERSION {
        return Err(StoreError::TooNew {
            found: current,
            known: SCHEMA_VERSION,
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
