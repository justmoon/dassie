# @dassie/lib-sqlite

A TypeScript library providing a framework for using SQLite synchronously with strong typing, declarative schema definition, and migration management, powered by `better-sqlite3` and `Kysely`.

## Features

-   **Declarative Schema Definition**: Define your database structure with `defineTable`, `defineColumn`, and `defineScalar` for clear and maintainable schemas.
-   **Strongly Typed**: Leverages TypeScript to provide type safety for your database interactions, including row types inferred from schema definitions.
-   **Synchronous API**: Built on `better-sqlite3` for fast, synchronous SQLite operations.
-   **Query Building with Kysely**: Utilizes Kysely for type-safe SQL query construction for complex queries (though basic operations use direct methods).
-   **Automatic Database Initialization**: Initializes the database according to the defined schema and applies any pending migrations.
-   **Migration Management**: Supports schema migrations to evolve your database structure over time.
-   **CRUD Operations**: Provides straightforward methods for creating, reading, updating, and deleting records.

## Installation

This package is intended for internal use within the Dassie monorepo and is typically consumed as a workspace dependency.

If you are developing within the Dassie monorepo, you can use `pnpm` to manage dependencies. For example, to add this library to another package in the workspace:

```bash
pnpm add @dassie/lib-sqlite --workspace
```

However, for direct use, ensure you have `pnpm` installed and dependencies are managed via the workspace configuration.

## Usage

### 1. Define your Schema

First, define the structure of your tables and scalar values.

```typescript
import {
  defineTable,
  defineColumn,
  defineScalar,
  type DatabaseSchema,
} from '@dassie/lib-sqlite';

// Define columns for a 'users' table
const UsersColumns = {
  id: defineColumn().type("INTEGER").primaryKey().autoIncrement(),
  name: defineColumn().type("TEXT").notNull(),
  email: defineColumn().type("TEXT").notNull().unique(),
};

// Define the 'users' table
const usersTable = defineTable({
  name: "users",
  columns: UsersColumns,
});

// Define a scalar value for database version
const databaseVersionScalar = defineScalar("database_version");

// Combine tables and scalars into a database schema object
const AppSchema = {
  applicationId: 0x_YOUR_APP_ID_HEX, // Replace with your unique 32-bit integer application ID (e.g. 0x1234ABCD)
  migrations: [], // See the "Migrations" section for how to populate this
  tables: {
    users: usersTable,
  },
  scalars: {
    databaseVersion: databaseVersionScalar,
  },
} as const satisfies DatabaseSchema; // `as const` and `satisfies` provide strong typing

// It's useful to export types for your table rows
export type User = typeof usersTable.inferRowType;
```
*Note: `applicationId` should be a unique 32-bit integer for your application (e.g., `0x1DA551E1`). The `0x_YOUR_APP_ID_HEX` is a placeholder.*

### 2. Create a Database Instance

Once your schema is defined, you can create a database instance.

```typescript
import { createDatabase } from '@dassie/lib-sqlite';
// Assuming AppSchema is defined as in the previous step

// Create (or open) the database file with the defined schema
const db = createDatabase({
  path: 'mydatabase.sqlite3', // Or ":memory:" for an in-memory database
  schema: AppSchema,
});
```

### 3. Basic Operations

Now you can interact with your database.

#### Inserting Data

```typescript
// db and AppSchema are from the previous steps

// Prepare data for insertion (id is auto-incrementing, so it's omitted)
const newUser: Omit<User, 'id'> = {
  name: 'Alice Wonderland',
  email: 'alice@example.com',
};

// Insert the data into the 'users' table
try {
  const insertedUser = db.tables.users.insertOne(newUser);
  console.log('Inserted user:', insertedUser);
} catch (error) {
  console.error("Failed to insert user:", error);
}

const anotherUser: Omit<User, 'id'> = {
  name: 'Bob The Builder',
  email: 'bob@example.com',
};
db.tables.users.insertOne(anotherUser);
```

#### Selecting Data

```typescript
// db and AppSchema are from the previous steps

const allUsers = db.tables.users.selectAll();
console.log('All users:', allUsers);

const alice = db.tables.users.selectOne({ where: { email: "alice@example.com" } });
console.log('Found Alice:', alice);

const userNames = db.tables.users.selectAll({
  columns: ["name"],
});
console.log('User names:', userNames);
```

#### Using Scalar Values

```typescript
// db and AppSchema are from the previous steps

db.scalars.databaseVersion.set(1);
console.log('Database version set to:', db.scalars.databaseVersion.get());

const currentVersion = db.scalars.databaseVersion.get() ?? 0;
db.scalars.databaseVersion.set(currentVersion + 1);
console.log('Database version updated to:', db.scalars.databaseVersion.get());
```

## Migrations

Database migrations are essential for managing the evolution of your database schema over time. As your application develops, you might need to add new tables, alter existing ones, or make other structural changes. Migrations provide a version-controlled way to apply these changes.

Each migration consists of `up` and `down` operations:
- `up`: Applies the changes for a specific version.
- `down`: Reverts the changes made by the `up` operation.

Migrations are run automatically when the database is initialized if the stored database version is older than the versions defined in the migrations array.

### `MigrationDefinition` Object

A migration is defined by an object conforming to the `MigrationDefinition` type:

-   `version: number`: A unique integer identifying the migration. Migrations are applied in ascending order of this version.
-   `up: (database: BetterSqlite3.Database) => void`: A function that executes SQL commands to apply the migration. It receives the `better-sqlite3` database instance.
-   `down: (database: BetterSqlite3.Database) => void`: A function that executes SQL commands to revert the migration.

### Example Migration File

It's common practice to define each migration in its own file, often numbered sequentially.

```typescript
// migrations/0001-create-posts-table.ts
import type { MigrationDefinition } from "@dassie/lib-sqlite";
import type { Database as BetterSqlite3Database } from "better-sqlite3";

const migration: MigrationDefinition = {
  version: 1,
  up: (database: BetterSqlite3Database) => {
    database.exec(`
      CREATE TABLE posts (
        id TEXT PRIMARY KEY,
        title TEXT NOT NULL,
        content TEXT
      ) STRICT, WITHOUT ROWID;
    `);
  },
  down: (database: BetterSqlite3Database) => {
    database.exec("DROP TABLE posts;");
  },
};

export default migration;
```

### Including Migrations in Your Schema

You would typically collect all your migration definitions into an array, often via an index file in your migrations directory (e.g., `migrations/index.ts`). This array is then passed to your `DatabaseSchema`.

```typescript
// schema.ts (or your relevant schema definition file)
import {
  type DatabaseSchema,
  defineTable,
  defineColumn,
  defineScalar, // Assuming you might have scalars too
} from "@dassie/lib-sqlite";

// Assume projectMigrations is an array of MigrationDefinition objects
// e.g., import migration1 from "./migrations/0001-create-posts-table";
// const projectMigrations = [migration1, /* ...other migrations */];
import projectMigrations from "./migrations"; // Adjust path as needed

// Example table from previous sections
const usersTable = defineTable({
  name: "users",
  columns: {
    id: defineColumn().type("INTEGER").primaryKey().autoIncrement(),
    name: defineColumn().type("TEXT").notNull(),
    email: defineColumn().type("TEXT").notNull().unique(),
  },
});

// Example scalar
const databaseVersionScalar = defineScalar("database_version");

export const AppSchemaWithMigrations: DatabaseSchema = {
  applicationId: 0x1DA551E1, // Replace with your actual application ID
  tables: {
    users: usersTable,
    // If the 'posts' table from the migration example is part of the latest schema,
    // it should also be defined here using defineTable.
    // posts: postsTable, // (Assuming postsTable is defined similarly)
  },
  scalars: {
    databaseVersion: databaseVersionScalar,
  },
  migrations: projectMigrations, // Assign the array of migrations here
};
```
**Important**: When you add a migration that creates or alters a table, ensure your main schema definition (the `tables` object in `DatabaseSchema`) reflects the *latest* state of the database after all migrations have been applied. The table definitions in the schema should not include the `CREATE TABLE` SQL directly; that's handled by the `up` functions in your migrations.

## Development

The following commands can be run from the root of the `packages/lib-sqlite` directory or using `pnpm --filter @dassie/lib-sqlite <command>`.

### Tests

Currently, there are no specific test scripts configured for this package beyond basic build checks. The `package.json` contains:
```bash
pnpm test # Outputs: Error: no test specified
```

### Linting

Lint your code using ESLint:

```bash
pnpm lint
```

### Type Checking

Check types using TypeScript compiler:

```bash
pnpm typecheck
```

## License

This library is licensed under the Apache-2.0 License. See the [LICENSE](../../../LICENSE) file for details.

## Author

-   Stefan Thomas ([https://justmoon.com/](https://justmoon.com/))

## Repository and Issues

-   Repository: [https://github.com/justmoon/dassie](https://github.com/justmoon/dassie) (specifically in `packages/lib-sqlite`)
-   Issues: [https://github.com/justmoon/dassie/issues](https://github.com/justmoon/dassie/issues)
```
