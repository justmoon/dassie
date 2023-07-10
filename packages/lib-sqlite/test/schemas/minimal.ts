import { DatabaseSchema, column, table } from "../../src"

export default {
  applicationId: 0x0e_ef_be_ef,
  migrations: [
    {
      version: 1,
      up: (database) => {
        database.exec(`
          CREATE TABLE users (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL
          )
        `)
      },
      down: (database) => {
        database.exec(`DROP TABLE users;`)
      },
    },
  ],
  tables: {
    users: table({
      name: "users",
      columns: {
        id: column().type("INTEGER").required().primaryKey(),
        name: column().type("TEXT").required(),
      },
    }),
  },
} satisfies DatabaseSchema
