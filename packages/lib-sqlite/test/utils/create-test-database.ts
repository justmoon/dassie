import {
  DatabaseSchema,
  InferDatabaseInstance,
  createDatabase,
} from "../../src"

export const createTestDatabase = <TSchema extends DatabaseSchema>(
  schema: TSchema
): InferDatabaseInstance<{
  schema: TSchema
}> => {
  const database = createDatabase({
    schema,
    path: ":memory:",
  })

  return database
}
