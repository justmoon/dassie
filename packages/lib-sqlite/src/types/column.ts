/* eslint-disable @typescript-eslint/no-explicit-any */
import { SqliteDataType, SqliteToTypescriptTypeMap } from "./sqlite-datatypes"

export interface ColumnDescriptionGenerics {
  sqliteType: SqliteDataType
  writeType: NonNullable<unknown>
  readType: NonNullable<unknown>
  notNull: boolean
  primaryKey: boolean
}

export interface DefaultColumnDescriptionGenerics {
  sqliteType: "TEXT"
  writeType: string
  readType: string
  notNull: false
  primaryKey: false
}

export interface ColumnDescription<
  T extends ColumnDescriptionGenerics = ColumnDescriptionGenerics
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
   * A transformation function that is applied when writing to the database.
   */
  serialize(
    this: void,
    value: T["writeType"]
  ): SqliteToTypescriptTypeMap[T["sqliteType"]]

  /**
   * A transformation function that is applied when reading from the database.
   */
  deserialize(
    this: void,
    value: SqliteToTypescriptTypeMap[T["sqliteType"]]
  ): T["readType"]
}

export interface ColumnDescriptionBuilder<
  T extends ColumnDescriptionGenerics = ColumnDescriptionGenerics
> {
  description: ColumnDescription<T>

  type<TType extends SqliteDataType>(
    type: TType
  ): ColumnDescriptionBuilder<
    Omit<T, "sqliteType" | "writeType" | "readType"> & {
      sqliteType: TType
      writeType: SqliteToTypescriptTypeMap[TType]
      readType: SqliteToTypescriptTypeMap[TType]
    }
  >

  serialize<TWriteType extends NonNullable<unknown>>(
    serializer: (
      value: TWriteType
    ) => SqliteToTypescriptTypeMap[T["sqliteType"]]
  ): ColumnDescriptionBuilder<
    Omit<T, "writeType"> & {
      writeType: TWriteType
    }
  >

  deserialize<TReadType extends NonNullable<unknown>>(
    deserializer: (
      value: SqliteToTypescriptTypeMap[T["sqliteType"]]
    ) => TReadType
  ): ColumnDescriptionBuilder<
    Omit<T, "readType"> & {
      readType: TReadType
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

export type InferColumnReadType<T extends ColumnDescription> =
  T extends ColumnDescription<infer TGenerics>
    ? TGenerics["readType"] | InferColumnNullable<T>
    : never

export type InferColumnWriteType<T extends ColumnDescription> =
  T extends ColumnDescription<infer TGenerics>
    ? TGenerics["writeType"] | InferColumnNullable<T>
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

export type InferRowReadTypeFromColumns<
  TColumns extends Record<string, ColumnDescription>
> = {
  [K in keyof TColumns]: InferColumnReadType<TColumns[K]>
}

export type InferRowWriteTypeFromColumns<
  TColumns extends Record<string, ColumnDescription>
> = {
  [K in keyof TColumns]: InferColumnWriteType<TColumns[K]>
}

export type InferRowSqliteTypeFromColumns<
  TColumns extends Record<string, ColumnDescription>
> = {
  [K in keyof TColumns]: SqliteToTypescriptTypeMap[TColumns[K]["type"]]
}

export type AnyColumnDescription = ColumnDescription<any>
export type AnyColumnDescriptionBuilder = ColumnDescriptionBuilder<any>
