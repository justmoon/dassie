import { type ScalarDescription } from "@dassie/lib-sqlite"

/**
 * This defines all the scalar values that are stored in the database which will
 * allow us to access them in a type-safe way.
 */
export const ACME_DATABASE_SCALARS = {
  "acme.account_key": {
    type: "TEXT",
  },
  "acme.account_url": {
    type: "TEXT",
  },
} as const satisfies Record<`acme.${string}`, ScalarDescription>
