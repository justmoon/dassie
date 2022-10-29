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
  | SqliteToTypescriptTypeMap[T["type"]]
  | T["defaultValue"]

export const scalarTable = {
  name: "scalar",
  columns: {
    key: { type: "TEXT" },
    value: { type: "ANY" },
  },
} as const

export const createScalarStore = <
  TScalars extends Record<string, ScalarDescription>
>(
  scalarDescriptions: TScalars,
  connectedScalarTable: ConnectedTable<typeof scalarTable>
): ScalarStore<TScalars> => {
  return {
    get(key) {
      if (!(key in scalarDescriptions)) {
        throw new Error(`Unknown scalar: ${key}`)
      }

      const value =
        connectedScalarTable.selectUnique("key", key)?.value ??
        scalarDescriptions[key]?.defaultValue

      return value as InferScalarType<TScalars[typeof key]>
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
