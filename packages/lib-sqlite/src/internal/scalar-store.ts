import type { ZodTypeAny, TypeOf as ZodTypeOf } from "zod"

import { column } from "../define-column"
import { table } from "../define-table"
import type {
  SqliteDataType,
  SqliteToTypescriptTypeMap,
} from "../types/sqlite-datatypes"
import type { ConnectedTable } from "./connect-table"

export type ScalarDescription<
  TSqliteType extends SqliteDataType = SqliteDataType
> =
  // This conditional ensures that the defaultValue is of the same type given
  // in the type property. Otherwise, the type system might infer TSqliteType
  // as a union of multiple sqlite types, which would allow default values of
  // the wrong type, e.g. { type: "INTEGER", defaultValue: "foo" }.
  TSqliteType extends SqliteDataType
    ? {
        type: TSqliteType
        defaultValue?: SqliteToTypescriptTypeMap[TSqliteType]
        schema?: ZodTypeAny
      }
    : never

export interface ScalarStore<
  TScalars extends Record<string, ScalarDescription>
> {
  get<TScalar extends keyof TScalars & string>(
    key: TScalar
  ): InferScalarType<TScalars[TScalar]>

  set<TScalar extends keyof TScalars & string>(
    key: TScalar,
    value: InferScalarType<TScalars[TScalar]>
  ): void
}

export type InferScalarType<T extends ScalarDescription> =
  | (T["schema"] extends ZodTypeAny
      ? ZodTypeOf<T["schema"]>
      : SqliteToTypescriptTypeMap[T["type"]])
  | ("defaultValue" extends keyof T ? T["defaultValue"] : undefined)

export const scalarTable = table({
  name: "scalar",
  columns: {
    key: column().type("TEXT").notNull().primaryKey(),
    value: column().type("ANY").notNull(),
  },
})

export const createScalarStore = <
  TScalars extends Record<string, ScalarDescription>
>(
  scalarDescriptions: TScalars,
  connectedScalarTable: ConnectedTable<typeof scalarTable>
): ScalarStore<TScalars> => {
  return {
    get(key) {
      const description = scalarDescriptions[key]
      if (!description) {
        throw new Error(`Unknown scalar: ${key}`)
      }

      const rawValue = connectedScalarTable.selectUnique("key", key)?.value
      // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
      const value: InferScalarType<TScalars[typeof key]> =
        rawValue === undefined
          ? description.defaultValue
          : description.schema
          ? description.schema.parse(rawValue)
          : rawValue

      // eslint-disable-next-line @typescript-eslint/no-unsafe-return
      return value
    },

    set(key, value) {
      if (!(key in scalarDescriptions)) {
        throw new Error(`Unknown scalar: ${key}`)
      }

      connectedScalarTable
        .insert()
        .upsert(["key"], ["value"])
        .one({ key, value })
    },
  }
}
