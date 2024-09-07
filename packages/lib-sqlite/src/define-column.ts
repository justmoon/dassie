import type {
  ColumnDescription,
  ColumnDescriptionBuilder,
  ColumnDescriptionGenerics,
  DefaultColumnDescriptionGenerics,
} from "./types/column"
import type { SqliteToTypescriptTypeMap } from "./types/sqlite-datatypes"

const createBuilder = <T extends ColumnDescriptionGenerics>(
  description: ColumnDescription<T>,
): ColumnDescriptionBuilder<T> => ({
  description,

  type(type) {
    if (description.hasDefault) {
      throw new Error("Cannot change column type after setting a default value")
    }

    const newDescription: ColumnDescription<
      Omit<T, "sqliteType" | "typescriptType"> & {
        sqliteType: typeof type
        typescriptType: SqliteToTypescriptTypeMap[typeof type]
      }
    > = {
      ...description,
      type,
      // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment, @typescript-eslint/no-explicit-any
      defaultValue: undefined as any,
    }

    return createBuilder(newDescription)
  },

  notNull() {
    const newDescription: ColumnDescription<
      Omit<T, "notNull"> & { notNull: true }
    > = {
      ...description,
      notNull: true,
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

  default(value: T["typescriptType"]) {
    const newDescription: ColumnDescription<
      Omit<T, "hasDefault"> & { hasDefault: true }
    > = {
      ...description,
      hasDefault: true,
      defaultValue: value,
    }
    return createBuilder(newDescription)
  },

  typescriptType() {
    // eslint-disable-next-line @typescript-eslint/no-unsafe-return, @typescript-eslint/no-explicit-any
    return createBuilder(description) as any
  },
})

export const column =
  (): ColumnDescriptionBuilder<DefaultColumnDescriptionGenerics> => {
    return createBuilder({
      type: "TEXT",
      notNull: false,
      primaryKey: false,
      hasDefault: false,
      defaultValue: undefined,
    })
  }
