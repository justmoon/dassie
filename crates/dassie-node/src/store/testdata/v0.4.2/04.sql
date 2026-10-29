
ALTER TABLE settlement_channels DROP COLUMN ended_for_room;

DROP INDEX ledger_transfers_by_packet;
DROP INDEX ledger_transfers_trust;
