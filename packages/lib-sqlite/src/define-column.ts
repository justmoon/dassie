import { SqliteToTypescriptTypeMap } from "."
import {
  ColumnDescription,
  ColumnDescriptionBuilder,
  ColumnDescriptionGenerics,
  DefaultColumnDescriptionGenerics,
} from "./types/column"

export const identity = <T>(value: T): T => value

const createBuilder = <T extends ColumnDescriptionGenerics>(
  description: ColumnDescription<T>
): ColumnDescriptionBuilder<T> => ({
  description,

  type(type) {
    const newDescription: ColumnDescription<
      Omit<T, "sqliteType" | "writeType" | "readType"> & {
        sqliteType: typeof type
        writeType: SqliteToTypescriptTypeMap[typeof type]
        readType: SqliteToTypescriptTypeMap[typeof type]
      }
    > = {
      ...description,
      type,
      serialize: identity,
      deserialize: identity,
    }

    return createBuilder(newDescription)
  },

  required() {
    const newDescription: ColumnDescription<
      Omit<T, "required"> & { required: true }
    > = {
      ...description,
      required: true,
    }
    return createBuilder(newDescription)
  },

  primaryKey() {
    const newDescription: ColumnDescription<
      Omit<T, "primaryKey"> & { primaryKey: true }
    > = {
      ...description,
      primaryKey: true,
    }
    return createBuilder(newDescription)
  },

  serialize(serialize) {
    const newDescription: ColumnDescription<
      Omit<T, "writeType"> & { writeType: Parameters<typeof serialize>[0] }
    > = {
      ...description,
      serialize,
    }
    return createBuilder(newDescription)
  },

  deserialize(deserialize) {
    const newDescription: ColumnDescription<
      Omit<T, "readType"> & { readType: ReturnType<typeof deserialize> }
    > = {
      ...description,
      deserialize,
    }
    return createBuilder(newDescription)
  },
})

export const column =
  (): ColumnDescriptionBuilder<DefaultColumnDescriptionGenerics> => {
    return createBuilder({
      type: "TEXT",
      required: false,
      primaryKey: false,
      serialize: identity,
      deserialize: identity,
    })
  }
