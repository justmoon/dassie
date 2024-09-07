import type { Simplify } from "type-fest"

import type {
  AnyColumnDescription,
  ColumnDescription,
  InferColumnRequired,
  InferColumnTypescriptType,
} from "./column"
import type { SqliteToTypescriptTypeMap } from "./sqlite-datatypes"

export interface TableDescription {
  name: string
  columns: Record<string, ColumnDescription>
}

export interface TableOptions {
  name: string
  columns: Record<string, { description: AnyColumnDescription }>
}

export interface InferTableDescription<T extends TableOptions = TableOptions>
  extends TableDescription {
  name: T["name"]
  columns: {
    [K in keyof T["columns"]]: T["columns"][K]["description"] extends (
      ColumnDescription<infer T>
    ) ?
      ColumnDescription<Simplify<T>>
    : never
  }
}

export type InferRow<T extends TableDescription> = {
  [K in keyof T["columns"]]: InferColumnTypescriptType<T["columns"][K]>
}

export type InferInsertRow<T extends TableDescription> = {
  [K in keyof T["columns"] as InferColumnRequired<T["columns"][K]> extends (
    false
  ) ?
    never
  : K]: InferColumnTypescriptType<T["columns"][K]>
} & {
  [K in keyof T["columns"] as InferColumnRequired<T["columns"][K]> extends (
    false
  ) ?
    K
  : never]?: InferColumnTypescriptType<T["columns"][K]>
}

export type InferRowSqliteType<T extends TableDescription> = {
  [K in keyof T["columns"]]: SqliteToTypescriptTypeMap[T["columns"][K]["type"]]
}

export type InferColumnNames<T extends TableDescription> = keyof T["columns"] &
  string
