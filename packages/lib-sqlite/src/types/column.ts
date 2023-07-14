/* eslint-disable @typescript-eslint/no-explicit-any */
import { SqliteDataType, SqliteToTypescriptTypeMap } from "./sqlite-datatypes"

export interface ColumnDescriptionGenerics {
  sqliteType: SqliteDataType
  writeType: unknown
  readType: unknown
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

  serialize<TWriteType>(
    serializer: (
      value: TWriteType
    ) => SqliteToTypescriptTypeMap[T["sqliteType"]]
  ): ColumnDescriptionBuilder<
    Omit<T, "writeType"> & {
      writeType: TWriteType
    }
  >

  deserialize<TReadType>(
    deserializer: (
      value: SqliteToTypescriptTypeMap[T["sqliteType"]]
    ) => TReadType
  ): ColumnDescriptionBuilder<
    Omit<T, "readType"> & {
      readType: TReadType
    }
  >

  notNull(): ColumnDescriptionBuilder<
    Omit<T, "required"> & {
      required: true
    }
  >

  primaryKey(): ColumnDescriptionBuilder<
    Omit<T, "primaryKey"> & {
      primaryKey: true
    }
  >
}

export type InferColumnReadType<T extends ColumnDescription> =
  T extends ColumnDescription<infer TGenerics> ? TGenerics["readType"] : never

export type InferColumnWriteType<T extends ColumnDescription> =
  T extends ColumnDescription<infer TGenerics> ? TGenerics["writeType"] : never

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
