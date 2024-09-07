import type {
  DefaultScalarDescriptionGenerics,
  ScalarDescription,
  ScalarDescriptionBuilder,
  ScalarDescriptionGenerics,
} from "./types/scalar"
import type { SqliteToTypescriptTypeMap } from "./types/sqlite-datatypes"
import { identity } from "./utils/identity"

const createBuilder = <T extends ScalarDescriptionGenerics>(
  description: ScalarDescription<T>,
): ScalarDescriptionBuilder<T> => ({
  description,

  name(name) {
    const newDescription: ScalarDescription<
      Omit<T, "name"> & { name: typeof name }
    > = {
      ...description,
      name,
    }
    return createBuilder(newDescription)
  },

  type(type) {
    const newDescription: ScalarDescription<
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

  serialize(serialize) {
    const newDescription: ScalarDescription<
      Omit<T, "writeType"> & { writeType: Parameters<typeof serialize>[0] }
    > = {
      ...description,
      serialize,
    }
    return createBuilder(newDescription)
  },

  deserialize(deserialize) {
    const newDescription: ScalarDescription<
      Omit<T, "readType"> & { readType: ReturnType<typeof deserialize> }
    > = {
      ...description,
      deserialize,
    }
    return createBuilder(newDescription)
  },

  defaultValue(defaultValue) {
    const newDescription: ScalarDescription<
      Omit<T, "defaultValueType"> & { defaultValueType: typeof defaultValue }
    > = {
      ...description,
      defaultValue,
    }
    return createBuilder(newDescription)
  },
})

export const scalar =
  (): ScalarDescriptionBuilder<DefaultScalarDescriptionGenerics> => {
    return createBuilder({
      name: undefined,
      type: "TEXT",
      serialize: identity,
      deserialize: identity,
      defaultValue: undefined,
    })
  }
