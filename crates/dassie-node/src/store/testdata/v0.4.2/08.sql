
ALTER TABLE peer_settings ADD COLUMN offer_swaps INTEGER NOT NULL DEFAULT 1
  CHECK (offer_swaps IN (0, 1));

ALTER TABLE peer_settings ADD COLUMN swapping INTEGER NOT NULL DEFAULT 1
  CHECK (swapping IN (0, 1));
