import {
  ColumnDescription,
  ColumnDescriptionBuilder,
  ColumnDescriptionGenerics,
  DefaultColumnDescriptionGenerics,
} from "./types/column"
import { SqliteToTypescriptTypeMap } from "./types/sqlite-datatypes"

const createBuilder = <T extends ColumnDescriptionGenerics>(
  description: ColumnDescription<T>,
): ColumnDescriptionBuilder<T> => ({
  description,

  type(type) {
    const newDescription: ColumnDescription<
      Omit<T, "sqliteType" | "typescriptType"> & {
        sqliteType: typeof type
        typescriptType: SqliteToTypescriptTypeMap[typeof type]
      }
    > = {
      ...description,
      type,
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
    })
  }
