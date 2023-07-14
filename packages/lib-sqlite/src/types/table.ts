import { Simplify } from "type-fest"

import { SqliteToTypescriptTypeMap } from ".."
import {
  AnyColumnDescription,
  ColumnDescription,
  InferColumnReadType,
  InferColumnWriteType,
} from "./column"

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
    [K in keyof T["columns"]]: T["columns"][K]["description"] extends ColumnDescription<
      infer T
    >
      ? ColumnDescription<Simplify<T>>
      : never
  }
}

export type InferRowReadType<T extends TableDescription> = {
  [K in keyof T["columns"]]: InferColumnReadType<T["columns"][K]>
}

export type InferRowWriteType<T extends TableDescription> = {
  [K in keyof T["columns"]]: InferColumnWriteType<T["columns"][K]>
}

export type InferRowSqliteType<T extends TableDescription> = {
  [K in keyof T["columns"]]: SqliteToTypescriptTypeMap[T["columns"][K]["type"]]
}

export type InferColumnNames<T extends TableDescription> = keyof T["columns"] &
  string
