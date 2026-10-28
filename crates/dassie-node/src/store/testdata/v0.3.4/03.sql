
ALTER TABLE peer_methods ADD COLUMN winding_since_ms INTEGER
  CHECK (winding_since_ms IS NULL OR winding_since_ms >= 0);

ALTER TABLE settlement_schemes ADD COLUMN removing_since_ms INTEGER
  CHECK (removing_since_ms IS NULL OR removing_since_ms >= 0);
