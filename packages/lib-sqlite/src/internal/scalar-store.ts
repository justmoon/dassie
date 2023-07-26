import { column } from "../define-column"
import { table } from "../define-table"
import {
  InferScalarReadType,
  InferScalarWriteType,
  ScalarDescriptionBuilder,
} from "../types/scalar"
import { identity } from "../utils/identity"
import type { ConnectedTable } from "./connect-table"

export interface ConnectedScalar<TScalar extends ScalarDescriptionBuilder> {
  get(): InferScalarReadType<TScalar>
  set(value: InferScalarWriteType<TScalar>): void
  delete(): void
}

export type ScalarStore<
  TScalars extends Record<string, ScalarDescriptionBuilder>
> = {
  [TScalar in keyof TScalars]: ConnectedScalar<TScalars[TScalar]>
}

export const scalarTable = table({
  name: "scalar",
  columns: {
    key: column().type("TEXT").notNull().primaryKey(),
    value: column().type("ANY").notNull(),
  },
})

export const createScalarStore = <
  TScalars extends Record<string, ScalarDescriptionBuilder>
>(
  scalarDescriptions: TScalars,
  connectedScalarTable: ConnectedTable<typeof scalarTable>
): ScalarStore<TScalars> => {
  const scalarStore = {} as ScalarStore<TScalars>

  for (const key in scalarDescriptions) {
    scalarStore[key] = createConnectedScalar(
      key,
      scalarDescriptions,
      connectedScalarTable
    )
  }

  return scalarStore
}

const createConnectedScalar = <
  TScalars extends Record<string, ScalarDescriptionBuilder>
>(
  key: keyof TScalars & string,
  scalarDescriptions: TScalars,
  connectedScalarTable: ConnectedTable<typeof scalarTable>
): ConnectedScalar<TScalars[typeof key]> => {
  return {
    get() {
      const description = scalarDescriptions[key]?.description
      if (!description) {
        throw new Error(`Unknown scalar: ${key}`)
      }

      const rawValue = connectedScalarTable.selectUnique("key", key)?.value
      const value =
        rawValue === undefined
          ? description.defaultValue
          : description.deserialize === identity
          ? rawValue
          : description.deserialize(rawValue)

      return value as InferScalarReadType<TScalars[typeof key]>
    },

    set(value) {
      const description = scalarDescriptions[key]?.description

      if (!description) {
        throw new Error(`Unknown scalar: ${key}`)
      }

      connectedScalarTable
        .insert()
        .upsert(["key"], ["value"])
        .one({
          key,
          value:
            description.serialize === identity
              ? value
              : description.serialize(value),
        })
    },

    delete() {
      if (!(key in scalarDescriptions)) {
        throw new Error(`Unknown scalar: ${key}`)
      }

      connectedScalarTable.select().where({ equals: { key } }).delete()
    },
  }
}
