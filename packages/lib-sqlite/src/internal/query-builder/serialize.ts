import { identity } from "../../define-column"
import {
  InferRowReadType,
  InferRowSqliteType,
  InferRowWriteType,
  TableDescription,
} from "../../types/table"

export type RowSerializer<T extends TableDescription> = (
  row: InferRowWriteType<T>
) => InferRowSqliteType<T>

export const createRowSerializer = <T extends TableDescription>(
  table: T
): RowSerializer<T> => {
  const serializers = new Map<string, (value: unknown) => unknown>()
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
        processedRow[columnName] =
          processedRow[columnName] == null
            ? null
            : serializer(processedRow[columnName])
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
  const deserializers = new Map<string, (value: unknown) => unknown>()
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
        processedRow[columnName] =
          processedRow[columnName] == null
            ? null
            : deserializer(processedRow[columnName])
      }
    }

    return processedRow as InferRowReadType<T>
  }
}
