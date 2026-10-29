
ALTER TABLE outgoing_payments ADD COLUMN delivered_amount INTEGER NOT NULL DEFAULT 0
  CHECK (delivered_amount >= 0);
