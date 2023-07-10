import {
  InferRowReadType,
  InferRowSqliteType,
  InferRowWriteType,
  TableDescription,
  TableDescriptionGenerics,
} from "../../types/table"

export type RowSerializer<
  T extends TableDescription<TableDescriptionGenerics>
> = (row: InferRowWriteType<T>) => InferRowSqliteType<T>

export const createRowSerializer = <
  T extends TableDescription<TableDescriptionGenerics>
>(
  table: T
): RowSerializer<T> => {
  const serializers = new Map<string, (value: unknown) => unknown>()
  for (const [columnName, column] of Object.entries(table.columns)) {
    if (column.description.serialize) {
      serializers.set(columnName, column.description.serialize)
    }
  }

  if (serializers.size === 0) {
    return (row) => row as InferRowSqliteType<T>
  }

  return (row) => {
    const processedRow = { ...row } as Record<string, unknown>

    for (const columnName of Object.keys(row)) {
      const serializer = serializers.get(columnName)

      if (serializer) {
        processedRow[columnName] = serializer(processedRow[columnName])
      }
    }

    return processedRow as InferRowSqliteType<T>
  }
}

export type RowDeserializer<
  T extends TableDescription<TableDescriptionGenerics>
> = (row: InferRowSqliteType<T>) => InferRowReadType<T>

export const createRowDeserializer = <
  T extends TableDescription<TableDescriptionGenerics>
>(
  table: T
): RowDeserializer<T> => {
  const deserializers = new Map<string, (value: unknown) => unknown>()
  for (const [columnName, column] of Object.entries(table.columns)) {
    if (column.description.deserialize) {
      deserializers.set(columnName, column.description.deserialize)
    }
  }

  if (deserializers.size === 0) {
    return (row) => row as InferRowReadType<T>
  }

  return (row) => {
    const processedRow = { ...row } as Record<string, unknown>

    for (const columnName of Object.keys(row)) {
      const deserializer = deserializers.get(columnName)

      if (deserializer) {
        processedRow[columnName] = deserializer(processedRow[columnName])
      }
    }

    return processedRow as InferRowReadType<T>
  }
}
