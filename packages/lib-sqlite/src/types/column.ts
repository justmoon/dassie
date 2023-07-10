/* eslint-disable @typescript-eslint/no-explicit-any */
import { SqliteDataType, SqliteToTypescriptTypeMap } from "./sqlite-datatypes"

export interface ColumnDescriptionGenerics {
  sqliteType: SqliteDataType
  writeType: unknown
  readType: unknown
  required: boolean
  primaryKey: boolean
}

export interface DefaultColumnDescriptionGenerics {
  sqliteType: "TEXT"
  writeType: string
  readType: string
  required: false
  primaryKey: false
}

export interface ColumnDescription<
  T extends ColumnDescriptionGenerics = ColumnDescriptionGenerics
> {
  type: T["sqliteType"]

  /**
   * Equivalent to `NOT NULL`.
   */
  required: T["required"]

  /**
   * Equivalent to `PRIMARY KEY`.
   */
  primaryKey: T["primaryKey"]

  /**
   * A transformation function that is applied when writing to the database.
   */
  serialize:
    | ((value: T["writeType"]) => SqliteToTypescriptTypeMap[T["sqliteType"]])
    | undefined

  /**
   * A transformation function that is applied when reading from the database.
   */
  deserialize:
    | ((value: SqliteToTypescriptTypeMap[T["sqliteType"]]) => T["readType"])
    | undefined
}

export interface ColumnDescriptionBuilder<
  T extends ColumnDescriptionGenerics = DefaultColumnDescriptionGenerics
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

  required(): ColumnDescriptionBuilder<
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
  | T["deserialize"] extends (value: unknown) => infer TReadType
  ? TReadType
  : SqliteToTypescriptTypeMap[T["type"]]

export type InferColumnWriteType<T extends ColumnDescription> =
  | T["serialize"] extends (value: infer TWriteType) => unknown
  ? TWriteType
  : SqliteToTypescriptTypeMap[T["type"]]

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
  [K in keyof TColumns]: TColumns[K]["type"]
}

export type AnyColumnDescriptionBuilder = ColumnDescriptionBuilder<any>
