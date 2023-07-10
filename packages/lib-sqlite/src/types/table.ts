import {
  AnyColumnDescriptionBuilder,
  InferColumnReadType,
  InferColumnWriteType,
} from "./column"

export interface TableDescriptionGenerics {
  name: string
  columns: TableColumnRecord
}

export interface TableDescription<T extends TableDescriptionGenerics> {
  name: T["name"]
  columns: T["columns"]
}

export type AnyTableDescription = TableDescription<TableDescriptionGenerics>

// eslint-disable-next-line @typescript-eslint/consistent-indexed-object-style
export interface TableColumnRecord {
  [columnName: string]: AnyColumnDescriptionBuilder
}

export type InferRowReadType<T extends AnyTableDescription> = {
  [K in keyof T["columns"]]: InferColumnReadType<T["columns"][K]["description"]>
}

export type InferRowWriteType<T extends AnyTableDescription> = {
  [K in keyof T["columns"]]: InferColumnWriteType<
    T["columns"][K]["description"]
  >
}

export type InferRowSqliteType<T extends AnyTableDescription> = {
  [K in keyof T["columns"]]: T["columns"][K]["description"]["type"]
}

export type InferColumnNames<T extends AnyTableDescription> =
  keyof T["columns"] & string
