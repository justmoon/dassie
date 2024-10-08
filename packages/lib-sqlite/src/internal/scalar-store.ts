import type { Database } from "better-sqlite3"

import { column } from "../define-column"
import { table } from "../define-table"
import type {
  InferScalarReadType,
  InferScalarSqliteType,
  InferScalarWriteType,
  ScalarDescriptionBuilder,
} from "../types/scalar"
import { identity } from "../utils/identity"

export interface ConnectedScalar<TScalar extends ScalarDescriptionBuilder> {
  get(): InferScalarReadType<TScalar>
  set(value: InferScalarWriteType<TScalar>): void
  delete(): void
}

export type ScalarStore<
  TScalars extends Record<string, ScalarDescriptionBuilder>,
> = {
  [TScalar in keyof TScalars]: ConnectedScalar<TScalars[TScalar]>
}

interface ScalarDriver {
  get(key: string): unknown
  set(key: string, value: unknown): void
  delete(key: string): void
}

export const scalarTable = table({
  name: "scalar",
  columns: {
    key: column().type("TEXT").notNull().primaryKey(),
    value: column().type("ANY").notNull(),
  },
  withoutRowid: true,
})

export const createScalarStore = <
  TScalars extends Record<string, ScalarDescriptionBuilder>,
>(
  scalarDescriptions: TScalars,
  database: Database,
): ScalarStore<TScalars> => {
  const scalarStore = {} as ScalarStore<TScalars>

  const getStatement = database.prepare(
    `SELECT value FROM scalar WHERE key = ?`,
  )
  const setStatement = database.prepare(
    `INSERT INTO scalar (key, value) VALUES (?, ?) ON CONFLICT (key) DO UPDATE SET value = excluded.value`,
  )
  const deleteStatement = database.prepare(`DELETE FROM scalar WHERE key = ?`)

  const driver: ScalarDriver = {
    get(key) {
      const row = getStatement.get(key) as { value: unknown } | undefined
      return row?.value
    },

    set(key, value) {
      setStatement.run(key, value)
    },

    delete(key) {
      deleteStatement.run(key)
    },
  }

  for (const key in scalarDescriptions) {
    scalarStore[key] = createConnectedScalar(key, scalarDescriptions, driver)
  }

  return scalarStore
}

const createConnectedScalar = <
  TScalars extends Record<string, ScalarDescriptionBuilder>,
>(
  key: keyof TScalars & string,
  scalarDescriptions: TScalars,
  driver: ScalarDriver,
): ConnectedScalar<TScalars[typeof key]> => {
  const description = scalarDescriptions[key]?.description
  if (!description) {
    throw new Error(`Unknown scalar: ${key}`)
  }

  return {
    get() {
      const rawValue = driver.get(key) as InferScalarSqliteType<
        TScalars[typeof key]
      >
      const value =
        rawValue === undefined ? description.defaultValue
        : description.deserialize === identity ? rawValue
        : description.deserialize(rawValue)

      return value as InferScalarReadType<TScalars[typeof key]>
    },

    set(value) {
      driver.set(
        key,
        description.serialize === identity ?
          value
        : description.serialize(value),
      )
    },

    delete() {
      driver.delete(key)
    },
  }
}
