import type SQLite from "better-sqlite3"

import { type DatabaseSchema, column, table } from "../../src"

export default {
  applicationId: 0x0e_ef_be_ef,
  migrations: [
    {
      version: 1,
      up: (database: SQLite.Database) => {
        database.exec(`
          CREATE TABLE users (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            age INTEGER NOT NULL
          )
        `)

        database.exec(`
          INSERT INTO users (id, name, age) VALUES
            (1, 'Anzhela', 42),
            (2, 'Borna', 21),
            (3, 'Caelius', 42)
        `)
      },
      down: (database: SQLite.Database) => {
        database.exec(`DROP TABLE users;`)
      },
    },
  ],
  tables: {
    users: table({
      name: "users",
      columns: {
        id: column().type("INTEGER").notNull().primaryKey(),
        name: column().type("TEXT").notNull(),
        age: column().type("INTEGER").notNull(),
      },
    }),
  },
} satisfies DatabaseSchema
