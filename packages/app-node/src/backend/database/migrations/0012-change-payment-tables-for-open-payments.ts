import type { MigrationDefinition } from "@dassie/lib-sqlite"

import m2 from "./0002-create-outgoing-payment-table"
import m3 from "./0003-create-incoming-payment-table"

const migration: MigrationDefinition = {
  version: 12,
  up: (database) => {
    database.prepare("DROP TABLE outgoing_payment").run()
    database.prepare("DROP TABLE incoming_payment").run()

    database
      .prepare(
        `
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
        `,
      )
      .run()

    database
      .prepare(
        `
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
        `,
      )
      .run()
  },
  down: (database) => {
    database.prepare("DROP TABLE outgoing_payment").run()
    database.prepare("DROP TABLE incoming_payment").run()
    m2.up(database)
    m3.up(database)
  },
}

export default migration
