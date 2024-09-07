import {
  type DatabaseInstance,
  type DatabaseSchema,
  createDatabase,
} from "../../src"

export const createTestDatabase = <TSchema extends DatabaseSchema>(
  schema: TSchema,
): DatabaseInstance<{
  schema: TSchema
}> => {
  const database = createDatabase({
    schema,
    path: ":memory:",
  })

  return database
}
