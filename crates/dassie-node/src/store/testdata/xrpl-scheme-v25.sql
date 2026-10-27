-- A database as the node wrote it at schema version 25, before external
-- assets, dumped as SQL: an xrpl-testnet scheme synced on the fake ledger,
-- with a settlement executed and in flight, one signed and booked but not
-- yet executed, and one it looks up by its hash. Its state is the scheme's
-- single blob, which migration 26 splits between the account and the scheme.
-- Its transactions are signed again by the wallet the node derives today
-- (spec/areas/identity.md#seed-paths), with their hashes, so that they are
-- the node's own, and its peer's node ID is written as IDs are today
-- (spec/areas/identity.md#node-id). Read by engine/core/tests.rs.
PRAGMA foreign_keys=OFF;
BEGIN TRANSACTION;
CREATE TABLE node_config (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  realm TEXT NOT NULL CHECK (realm IN ('test', 'live')),
  hostname TEXT CHECK (hostname IS NULL OR (length(hostname) BETWEEN 1 AND 253 AND hostname = lower(hostname))),
  http_port INTEGER NOT NULL CHECK (http_port BETWEEN 1 AND 65535),
  https_port INTEGER NOT NULL CHECK (https_port BETWEEN 1 AND 65535 AND https_port != http_port),
  alias TEXT NOT NULL CHECK (length(CAST(alias AS BLOB)) <= 64),
  acme_directory_url TEXT NOT NULL CHECK (acme_directory_url LIKE 'https://%')
) STRICT;
INSERT INTO node_config VALUES(1,'test',NULL,80,443,'','https://acme-v02.api.letsencrypt.org/directory');
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
CREATE TABLE boot (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  epoch INTEGER NOT NULL CHECK (epoch >= 0)
) STRICT;
INSERT INTO boot VALUES(1,1);
CREATE TABLE ledger_accounts (
  id INTEGER PRIMARY KEY CHECK (id >= 0),
  path TEXT NOT NULL UNIQUE,
  debits_posted INTEGER NOT NULL DEFAULT 0 CHECK (debits_posted >= 0),
  credits_posted INTEGER NOT NULL DEFAULT 0 CHECK (credits_posted >= 0)
) STRICT;
INSERT INTO ledger_accounts VALUES(0,'xrpl-testnet+xrp:assets/settlement',999000000000,0);
INSERT INTO ledger_accounts VALUES(1,'xrpl-testnet+xrp:equity/owner',0,999000000000);
INSERT INTO ledger_accounts VALUES(2,'xrpl-testnet+xrp:equity/suspense',0,0);
INSERT INTO ledger_accounts VALUES(3,'xrpl-testnet+xrp:revenue/fx',0,0);
INSERT INTO ledger_accounts VALUES(4,'xrpl-testnet+xrp:revenue/fees',0,0);
INSERT INTO ledger_accounts VALUES(5,'xrpl-testnet+xrp:expenses/fx',0,0);
INSERT INTO ledger_accounts VALUES(6,'xrpl-testnet+xrp:expenses/fees',0,0);
INSERT INTO ledger_accounts VALUES(7,'xrpl-testnet+xrp:expenses/losses',0,0);
INSERT INTO ledger_accounts VALUES(8,'xrpl-testnet+xrp:equity/owner/local/pay',0,0);
INSERT INTO ledger_accounts VALUES(9,'xrpl-testnet+xrp:assets/interledger/uvmpoarumy7tpcrcr4qrnzx77tm2uzin3a3p7ja',0,500000000000);
INSERT INTO ledger_accounts VALUES(10,'xrpl-testnet+xrp:contra/trust/uvmpoarumy7tpcrcr4qrnzx77tm2uzin3a3p7ja',500000000000,0);
CREATE TABLE settlement_schemes (
  id TEXT PRIMARY KEY,
  config TEXT NOT NULL,
  activated_at_ms INTEGER NOT NULL
) STRICT, WITHOUT ROWID;
INSERT INTO settlement_schemes VALUES('xrpl-testnet','{"server":"ws://ledger.test","faucet":null}',1800000000000);
CREATE TABLE settlement_checkpoints (
  scheme TEXT PRIMARY KEY,
  balance INTEGER NOT NULL CHECK (balance >= 0),
  updated_at_ms INTEGER NOT NULL
, state BLOB
  CHECK (state IS NULL OR length(state) <= 262144)) STRICT, WITHOUT ROWID;
INSERT INTO settlement_checkpoints VALUES('xrpl-testnet',999000000000,1800000000000,x'7b22637572736f72223a5b353030303030312c343239343936373239355d2c22696e5f666c69676874223a5b7b22626c6f62223a223132303030303232303030303030303032343030344334423431323031423030344334423535363134303030303030303031333132443030363834303030303030303030303030303041373332314544384245413244333233334538443832324543433136314536383730344444304442424438424538343136323143354338433832453934433935343744433446373734343034343431323435323336363536453130313736414638434337363542434136453144363231423832303044343835444336303842434138444345384336454639394432323245393238334135454643363242344533353043313744433933453430343933384531434344444441413438394439454230333330313333444530373831313436343737303044384145343043333834303533453333304430354539374332374636423343364335383331344635394138303339433430424631413641444637434543343337344336343439434244304631313446394541374332443638373437343730373333413246324636343631373337333639363532453643363136453634324637333635373437343643363536443635364537343246373837323730324437343635373337343645363537343246373633313744323136343531363833303741354136423737333136393437373035343738344534383437373934343432344337343532364135413332343732443434343235463442354145314631222c2264726f7073223a32303030303030302c226578656375746564223a747275652c22666565223a31302c226c6173745f6c6564676572223a353030303032312c2273657175656e6365223a353030303030312c227375626d69747465645f6174223a353030303030317d2c7b22626c6f62223a223132303030303232303030303030303032343030344334423432323031423030344334423535363134303030303030303031433943333830363834303030303030303030303030303041373332314544384245413244333233334538443832324543433136314536383730344444304442424438424538343136323143354338433832453934433935343744433446373734343044463244463134464232354643303631423935344234343435334543464345353344454638394442453239344645454535423146373031454233344139434132333844373731414231453138453842363337394434303343454142393536303646463137363246444332423639353038323837413037313139463432393430323831313436343737303044384145343043333834303533453333304430354539374332374636423343364335383331344635394138303339433430424631413641444637434543343337344336343439434244304631313446394541374332443638373437343730373333413246324636343631373337333639363532453643363136453634324637333635373437343643363536443635364537343246373837323730324437343635373337343645363537343246373633313744323136343531363833303741354136423737333136393437373035343738344534383437373934343432344337343532364135413332343732443434343235463442354145314631222c2264726f7073223a33303030303030302c226578656375746564223a66616c73652c22666565223a31302c226c6173745f6c6564676572223a353030303032312c2273657175656e6365223a353030303030322c227375626d69747465645f6174223a353030303030317d5d2c226e6577657374223a5b353030303030312c3830303030303030345d2c227065657273223a7b2275766d706f6172756d79377470637263723471726e7a783737746d32757a696e33613370376a61223a2272505064647543394d525472585a50314a374d5179454b4b455969466967575a3651227d2c227265636f766572696e67223a5b2241424142414241424142414241424142414241424142414241424142414241424142414241424142414241424142414241424142414241424142414241424142225d2c2273657175656e6365223a353030303030337d');
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
  credit_out INTEGER NOT NULL CHECK (credit_out >= 0),
  since_ms INTEGER NOT NULL,
  inbound INTEGER NOT NULL CHECK (inbound IN (0, 1)),
  last_seen_ms INTEGER NOT NULL,
  -- The newest timestamp accepted from the peer: after a restart, nothing
  -- at or below it is accepted again.
  short_for_ms INTEGER NOT NULL DEFAULT 0, depeering_since_ms INTEGER, depeering_reason INTEGER
  CHECK (depeering_reason IS NULL OR depeering_reason BETWEEN 0 AND 255), short_reached_ms INTEGER NOT NULL DEFAULT 0) STRICT, WITHOUT ROWID;
CREATE TABLE own_link_state (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  sequence INTEGER NOT NULL CHECK (sequence >= 0)
) STRICT;
INSERT INTO own_link_state VALUES(1,0);
CREATE TABLE peer_terminations (
  node TEXT PRIMARY KEY REFERENCES peers (node) ON DELETE CASCADE,
  since_ms INTEGER NOT NULL,
  reason INTEGER NOT NULL CHECK (reason BETWEEN 0 AND 255),
  by_us INTEGER NOT NULL CHECK (by_us IN (0, 1)),
  acknowledged INTEGER NOT NULL CHECK (acknowledged IN (0, 1))
) STRICT, WITHOUT ROWID;
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
  outbox_attempts INTEGER CHECK (outbox_attempts IS NULL OR outbox_attempts >= 0), ended_for_room INTEGER NOT NULL DEFAULT 0
  CHECK (ended_for_room IN (0, 1)),
  CHECK ((outbox_sequence IS NULL) = (outbox_scheme IS NULL)
     AND (outbox_scheme IS NULL) = (outbox_amount IS NULL)
     AND (outbox_amount IS NULL) = (outbox_data IS NULL)
     AND (outbox_data IS NULL) = (outbox_attempts IS NULL))
) STRICT, WITHOUT ROWID;
CREATE TABLE peer_bans (
  node TEXT PRIMARY KEY CHECK (length(node) BETWEEN 2 AND 45)
) STRICT, WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS "btp_tokens" (
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
, network BLOB
  CHECK (network IS NULL OR length(network) = 16)) STRICT, WITHOUT ROWID;
CREATE TABLE gnap_tokens (
  token_hash BLOB PRIMARY KEY CHECK (length(token_hash) = 32),
  grant_id BLOB NOT NULL REFERENCES gnap_grants (id) ON DELETE CASCADE,
  manage_id BLOB NOT NULL UNIQUE CHECK (length(manage_id) = 16),
  created_at_ms INTEGER NOT NULL,
  expires_at_ms INTEGER NOT NULL CHECK (expires_at_ms > created_at_ms)
) STRICT, WITHOUT ROWID;
CREATE TABLE payment_settings (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  -- The ledger `$host` names; NULL for the lowest active one.
  default_ledger TEXT
, display_currency TEXT
  CHECK (display_currency IS NULL OR length(display_currency) BETWEEN 1 AND 8)) STRICT;
INSERT INTO payment_settings VALUES(1,NULL,NULL);
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
, recipient TEXT
  CHECK (recipient IS NULL OR length(recipient) BETWEEN 1 AND 2048), fixed_debit INTEGER NOT NULL DEFAULT 0
  CHECK (fixed_debit IN (0, 1))) STRICT, WITHOUT ROWID;
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
, recipient TEXT
  CHECK (recipient IS NULL OR length(recipient) BETWEEN 1 AND 2048), fixed_debit INTEGER NOT NULL DEFAULT 0
  CHECK (fixed_debit IN (0, 1))) STRICT, WITHOUT ROWID;
CREATE TABLE grant_spend (
  grant_id BLOB NOT NULL CHECK (length(grant_id) = 16),
  -- The start of the window of the grant's interval; 0 without one.
  window_start_ms INTEGER NOT NULL CHECK (window_start_ms >= 0),
  debit_amount INTEGER NOT NULL CHECK (debit_amount >= 0),
  receive_amount INTEGER NOT NULL CHECK (receive_amount >= 0),
  PRIMARY KEY (grant_id, window_start_ms)
) STRICT, WITHOUT ROWID;
CREATE TABLE peer_settings (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  connector_fee_ppm INTEGER NOT NULL DEFAULT 0
  CHECK (connector_fee_ppm BETWEEN 0 AND 100000), fx_spread_ppm INTEGER NOT NULL DEFAULT 0
  CHECK (fx_spread_ppm BETWEEN 0 AND 100000)) STRICT;
INSERT INTO peer_settings VALUES(1,0,0);
CREATE TABLE IF NOT EXISTS "ledger_transfers" (
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
INSERT INTO ledger_transfers VALUES(0,0,1,999000000000,'posted',1800000000000,1800000000000,'deposit',NULL,NULL,NULL,NULL,1);
INSERT INTO ledger_transfers VALUES(1,10,9,500000000000,'posted',1800000000000,1800000000000,'trust',NULL,NULL,NULL,NULL,2);
INSERT INTO ledger_transfers VALUES(2,9,0,20000000000,'pending',1800000000000,NULL,'settlement',NULL,NULL,'xrpl-testnet','95EF1A29CF4CCDFBD4C104FAFB39272EE1713FE200CA02BA8272C89AA876E109',3);
INSERT INTO ledger_transfers VALUES(3,9,0,30000000000,'pending',1800000000000,NULL,'settlement',NULL,NULL,'xrpl-testnet','0A9D5BD48CE1F4DDC244D538B2638499CE0B29473A422267104E6EFD61A6D666',4);
INSERT INTO ledger_transfers VALUES(4,9,0,5000000000,'pending',1800000000000,NULL,'settlement',NULL,NULL,'xrpl-testnet','ABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABAB',5);
CREATE TABLE exchange_rates (
  asset TEXT PRIMARY KEY CHECK (length(asset) BETWEEN 1 AND 8),
  -- Units of the asset one US dollar buys, times 10^15, in decimal: it may
  -- not fit 64 bits.
  rate TEXT NOT NULL CHECK (length(rate) BETWEEN 1 AND 39
    AND rate NOT GLOB '*[^0-9]*' AND rate NOT GLOB '0*'),
  as_of_ms INTEGER NOT NULL CHECK (as_of_ms >= 0)
) STRICT, WITHOUT ROWID;
CREATE TABLE incoming_connections (
  incoming_payment BLOB NOT NULL REFERENCES incoming_payments (id) ON DELETE CASCADE,
  -- Which credentials of the payment the connection uses; zeros for those
  -- issued with the payment before connections had keys.
  connection BLOB NOT NULL CHECK (length(connection) = 16),
  received INTEGER NOT NULL CHECK (received > 0),
  PRIMARY KEY (incoming_payment, connection)
) STRICT, WITHOUT ROWID;
CREATE TABLE setup_token (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  token BLOB NOT NULL CHECK (length(token) = 32)
) STRICT;
CREATE TABLE IF NOT EXISTS "rate_sources" (
  position INTEGER PRIMARY KEY CHECK (position BETWEEN 0 AND 7),
  kind TEXT NOT NULL CHECK (kind IN ('coinbase', 'kraken', 'bitstamp', 'ecb')),
  -- Only the Coinbase format is served by others too.
  url TEXT CHECK (url IS NULL OR (kind = 'coinbase' AND length(url) BETWEEN 9 AND 2048))
) STRICT;
INSERT INTO rate_sources VALUES(0,'coinbase',NULL);
INSERT INTO rate_sources VALUES(1,'kraken',NULL);
INSERT INTO rate_sources VALUES(2,'bitstamp',NULL);
INSERT INTO rate_sources VALUES(3,'ecb',NULL);
CREATE TABLE replay_floors (
  node TEXT PRIMARY KEY CHECK (length(node) BETWEEN 2 AND 45),
  floor_ms INTEGER NOT NULL
) STRICT, WITHOUT ROWID;
CREATE INDEX sessions_by_last_use ON sessions (last_used_at_ms);
CREATE INDEX incoming_payments_by_ledger ON incoming_payments (ledger, created_at_ms, id);
CREATE INDEX incoming_payments_by_client ON incoming_payments (ledger, client, created_at_ms, id);
CREATE INDEX incoming_payments_by_time ON incoming_payments (created_at_ms, id);
CREATE INDEX incoming_payments_open ON incoming_payments (expires_at_ms) WHERE completed_at_ms IS NULL;
CREATE INDEX incoming_payments_unpaid ON incoming_payments (coalesce(completed_at_ms, expires_at_ms))
  WHERE received_amount = 0;
CREATE INDEX gnap_grants_by_state ON gnap_grants (state, created_at_ms);
CREATE INDEX gnap_tokens_by_grant ON gnap_tokens (grant_id);
CREATE INDEX gnap_tokens_by_expiry ON gnap_tokens (expires_at_ms);
CREATE INDEX quotes_by_expiry ON quotes (expires_at_ms);
CREATE INDEX outgoing_payments_by_ledger ON outgoing_payments (ledger, created_at_ms, id);
CREATE INDEX outgoing_payments_by_client ON outgoing_payments (ledger, client, created_at_ms, id);
CREATE INDEX outgoing_payments_by_time ON outgoing_payments (created_at_ms, id);
CREATE INDEX outgoing_payments_sending ON outgoing_payments (created_at_ms) WHERE state = 'sending';
CREATE INDEX ledger_transfers_by_debit ON ledger_transfers (debit, state);
CREATE INDEX ledger_transfers_by_credit ON ledger_transfers (credit, state);
CREATE INDEX ledger_transfers_pending ON ledger_transfers (state) WHERE state = 'pending';
CREATE INDEX ledger_transfers_by_debit_sequence ON ledger_transfers (debit, sequence);
CREATE INDEX ledger_transfers_by_credit_sequence ON ledger_transfers (credit, sequence);
CREATE INDEX ledger_transfers_refusals ON ledger_transfers (credit, created_at_ms)
  WHERE reference = 'refusal';
CREATE INDEX outgoing_payments_by_destination ON outgoing_payments (destination);
CREATE INDEX ledger_transfers_by_packet ON ledger_transfers (packet_epoch, packet_sequence)
  WHERE packet_epoch IS NOT NULL;
CREATE INDEX ledger_transfers_trust ON ledger_transfers (sequence) WHERE reference = 'trust';
COMMIT;
PRAGMA application_id = 2108023497;
PRAGMA user_version = 25;
