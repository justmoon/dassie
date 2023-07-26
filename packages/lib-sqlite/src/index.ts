// eslint-disable-next-line @typescript-eslint/triple-slash-reference
/// <reference path="types/better-sqlite3.d.ts" />

export * from "./create-database"
export * from "./define-table"
export * from "./define-column"
export * from "./define-scalar"
export type { MigrationDefinition } from "./types/migration"
export type * from "./types/table"
export type * from "./types/column"
export type * from "./types/scalar"
export type * from "./types/sqlite-datatypes"

export { identity } from "./utils/identity"
