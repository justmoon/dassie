
ALTER TABLE hub_funding ADD COLUMN target INTEGER
  CHECK (target IS NULL OR target > 0);
