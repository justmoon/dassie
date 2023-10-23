/* eslint-disable @typescript-eslint/no-explicit-any */
import { SqliteDataType, SqliteToTypescriptTypeMap } from "./sqlite-datatypes"

export interface ColumnDescriptionGenerics {
  sqliteType: SqliteDataType
  typescriptType: NonNullable<unknown>
  notNull: boolean
  primaryKey: boolean
}

export interface DefaultColumnDescriptionGenerics {
  sqliteType: "TEXT"
  typescriptType: string
  notNull: false
  primaryKey: false
}

export interface ColumnDescription<
  T extends ColumnDescriptionGenerics = ColumnDescriptionGenerics,
> {
  type: T["sqliteType"]

  /**
   * Equivalent to `NOT NULL`.
   */
  notNull: T["notNull"]

  /**
   * Equivalent to `PRIMARY KEY`.
   */
  primaryKey: T["primaryKey"]
}

export interface ColumnDescriptionBuilder<
  T extends ColumnDescriptionGenerics = ColumnDescriptionGenerics,
> {
  description: ColumnDescription<T>

  type<TType extends SqliteDataType>(
    type: TType,
  ): ColumnDescriptionBuilder<
    Omit<T, "sqliteType" | "typescriptType"> & {
      sqliteType: TType
      typescriptType: SqliteToTypescriptTypeMap[TType]
    }
  >

  typescriptType<
    TTypescriptType extends SqliteToTypescriptTypeMap[T["sqliteType"]],
  >(): ColumnDescriptionBuilder<
    Omit<T, "typescriptType"> & {
      typescriptType: TTypescriptType
    }
  >

  notNull(): ColumnDescriptionBuilder<
    Omit<T, "notNull"> & {
      notNull: true
    }
  >

  primaryKey(): ColumnDescriptionBuilder<
    Omit<T, "primaryKey"> & {
      primaryKey: true
    }
  >
}

export type InferColumnTypescriptType<T extends ColumnDescription> =
  T extends ColumnDescription<infer TGenerics>
    ? TGenerics["typescriptType"] | InferColumnNullable<T>
    : never

export type InferColumnSqliteType<T extends ColumnDescription> =
  T extends ColumnDescription<infer TGenerics>
    ?
        | SqliteToTypescriptTypeMap[TGenerics["sqliteType"]]
        | InferColumnNullable<T>
    : never

export type InferColumnNullable<T extends ColumnDescription> =
  T extends ColumnDescription<infer TGenerics>
    ? TGenerics["notNull"] extends false
      ? TGenerics["primaryKey"] extends false
        ? null
        : never
      : never
    : never

export type AnyColumnDescription = ColumnDescription<any>
export type AnyColumnDescriptionBuilder = ColumnDescriptionBuilder<any>
