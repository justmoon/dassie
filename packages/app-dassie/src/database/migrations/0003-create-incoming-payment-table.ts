import type { MigrationDefinition } from "@dassie/lib-sqlite"

const migration: MigrationDefinition = {
  version: 3,
  up: (database) => {
    database
      .prepare(
        `
          CREATE TABLE incoming_payment (
            -- The public unique ID representing this payment
            id TEXT NOT NULL PRIMARY KEY,

            -- Which subnet we are receiving this payment on
            subnet TEXT NOT NULL,

            -- Total amount to send in fundamental units of the given subnet
            total_amount INTEGER NOT NULL,

            -- Amount received so far in fundamental units of the given subnet
            received_amount INTEGER NOT NULL,

            -- External payment ID or similar identifying information
            external_reference TEXT NOT NULL
          ) STRICT
        `,
      )
      .run()
  },
  down: (database) => {
    database.prepare(`DROP TABLE incoming_payment`).run()
  },
}

export default migration
