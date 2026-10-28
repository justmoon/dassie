
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
