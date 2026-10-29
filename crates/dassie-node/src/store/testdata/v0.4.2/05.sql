
ALTER TABLE hub_escrow ADD COLUMN verifier BLOB
  CHECK (verifier IS NULL OR length(verifier) <= 262144);
