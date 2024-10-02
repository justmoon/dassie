import type { MigrationDefinition } from "@dassie/lib-sqlite"

import { CREATE_R2_OUTGOING_PAYMENTS_TABLE } from "./0002-create-outgoing-payment-table"
import { CREATE_R3_INCOMING_PAYMENTS_TABLE } from "./0003-create-incoming-payment-table"

export const CREATE_R12_OUTGOING_PAYMENTS_TABLE = `
CREATE TABLE outgoing_payment (
  -- Unique Payment ID
  id TEXT PRIMARY KEY NOT NULL,

  -- Destination Payment URL
  destination TEXT NOT NULL,

  -- Internal ledger containing the payment account
  ledger TEXT NOT NULL,

  -- Total amount to send in fundamental units of the given ledger
  total_amount INTEGER NOT NULL,

  -- Metadata about the payment in JSON format
  metadata TEXT NOT NULL
) STRICT
`

export const CREATE_R12_INCOMING_PAYMENTS_TABLE = `
CREATE TABLE incoming_payment (
  -- Unique Payment ID
  id TEXT PRIMARY KEY NOT NULL,

  -- Internal ledger containing the payment account
  ledger TEXT NOT NULL,

  -- Total amount to send in fundamental units of the given ledger
  total_amount INTEGER NOT NULL,

  -- Metadata about the payment in JSON format
  metadata TEXT NOT NULL
) STRICT
`

const migration: MigrationDefinition = {
  version: 12,
  up: (database) => {
    database.prepare("DROP TABLE outgoing_payment").run()
    database.prepare("DROP TABLE incoming_payment").run()

    database.prepare(CREATE_R12_OUTGOING_PAYMENTS_TABLE).run()
    database.prepare(CREATE_R12_INCOMING_PAYMENTS_TABLE).run()
  },
  down: (database) => {
    database.prepare("DROP TABLE outgoing_payment").run()
    database.prepare("DROP TABLE incoming_payment").run()
    database.prepare(CREATE_R2_OUTGOING_PAYMENTS_TABLE).run()
    database.prepare(CREATE_R3_INCOMING_PAYMENTS_TABLE).run()
  },
}

export default migration
