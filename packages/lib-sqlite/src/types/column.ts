/* eslint-disable @typescript-eslint/no-explicit-any */
import type {
  SqliteDataType,
  SqliteToTypescriptTypeMap,
} from "./sqlite-datatypes"

export interface ColumnDescriptionGenerics {
  sqliteType: SqliteDataType
  typescriptType: NonNullable<unknown>
  notNull: boolean
  primaryKey: boolean
  hasDefault: boolean
}

export interface DefaultColumnDescriptionGenerics {
  sqliteType: "TEXT"
  typescriptType: string
  notNull: false
  primaryKey: false
  hasDefault: false
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

  /**
   * Equivalent to `DEFAULT`.
   */
  hasDefault: T["hasDefault"]
  defaultValue: T["hasDefault"] extends true ? T["typescriptType"] : undefined
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

  default(defaultValue: T["typescriptType"]): ColumnDescriptionBuilder<
    Omit<T, "hasDefault"> & {
      hasDefault: true
    }
  >
}

export type InferColumnTypescriptType<T extends ColumnDescription> =
  T extends ColumnDescription<infer TGenerics> ?
    TGenerics["typescriptType"] | InferColumnNullable<T>
  : never

export type InferColumnSqliteType<T extends ColumnDescription> =
  T extends ColumnDescription<infer TGenerics> ?
    SqliteToTypescriptTypeMap[TGenerics["sqliteType"]] | InferColumnNullable<T>
  : never

export type InferColumnRequired<T extends ColumnDescription> =
  T extends ColumnDescription<infer TGenerics> ?
    TGenerics["hasDefault"] extends true ? false
    : TGenerics["notNull"] extends false ?
      TGenerics["primaryKey"] extends false ?
        false
      : true
    : true
  : true

export type InferColumnNullable<T extends ColumnDescription> =
  T extends ColumnDescription<infer TGenerics> ?
    TGenerics["notNull"] extends false ?
      TGenerics["primaryKey"] extends false ?
        null
      : never
    : never
  : never

export type AnyColumnDescription = ColumnDescription<any>
export type AnyColumnDescriptionBuilder = ColumnDescriptionBuilder<any>
