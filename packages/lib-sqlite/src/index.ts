// eslint-disable-next-line @typescript-eslint/triple-slash-reference
/// <reference path="types/better-sqlite3.d.ts" />

export * from "./create-database"
export * from "./define-table"
export type { MigrationDefinition } from "./types/migration"
export type {
  ScalarDescription,
  InferScalarType,
} from "./internal/scalar-store"
export type * from "./types/sqlite-datatypes"
