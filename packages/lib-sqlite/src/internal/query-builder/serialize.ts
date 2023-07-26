import {
  InferRowReadType,
  InferRowSqliteType,
  InferRowWriteType,
  TableDescription,
} from "../../types/table"
import { identity } from "../../utils/identity"

export type RowSerializer<T extends TableDescription> = (
  row: InferRowWriteType<T>
) => InferRowSqliteType<T>

export const createRowSerializer = <T extends TableDescription>(
  table: T
): RowSerializer<T> => {
  const serializers = new Map<
    string,
    (this: void, value: NonNullable<unknown>) => unknown
  >()
  for (const [columnName, column] of Object.entries(table.columns)) {
    if (column.serialize !== identity) {
      serializers.set(columnName, column.serialize)
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
        const value = processedRow[columnName]
        processedRow[columnName] = value == null ? null : serializer(value)
      }
    }

    return processedRow as InferRowSqliteType<T>
  }
}

export type RowDeserializer<T extends TableDescription> = (
  row: InferRowSqliteType<T>
) => InferRowReadType<T>

export const createRowDeserializer = <T extends TableDescription>(
  table: T
): RowDeserializer<T> => {
  const deserializers = new Map<
    string,
    (value: NonNullable<unknown>) => unknown
  >()
  for (const [columnName, column] of Object.entries(table.columns)) {
    if (column.deserialize !== identity) {
      deserializers.set(columnName, column.deserialize)
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
        const value = processedRow[columnName]
        processedRow[columnName] = value == null ? null : deserializer(value)
      }
    }

    return processedRow as InferRowReadType<T>
  }
}
