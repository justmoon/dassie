import type { MigrationDefinition } from "@dassie/lib-sqlite"

export const CREATE_R2_OUTGOING_PAYMENTS_TABLE = `
CREATE TABLE outgoing_payment (
  -- Destination Payment Pointer
  destination TEXT NOT NULL,

  -- Which subnet we are sending from
  subnet TEXT NOT NULL,

  -- Total amount to send in fundamental units of the given subnet
  total_amount INTEGER NOT NULL,

  -- Amount sent so far in fundamental units of the given subnet
  sent_amount INTEGER NOT NULL
) STRICT
`

const migration: MigrationDefinition = {
  version: 2,
  up: (database) => {
    database.prepare(CREATE_R2_OUTGOING_PAYMENTS_TABLE).run()
  },
  down: (database) => {
    database.prepare(`DROP TABLE outgoing_payment`).run()
  },
}

export default migration
