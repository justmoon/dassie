
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
