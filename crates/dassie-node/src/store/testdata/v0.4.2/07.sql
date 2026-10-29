
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
