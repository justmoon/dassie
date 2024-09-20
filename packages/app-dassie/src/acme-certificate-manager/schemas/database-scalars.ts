import { type NamespacedScalars, scalar } from "@dassie/lib-sqlite"

/**
 * This defines all the scalar values that are stored in the database which will
 * allow us to access them in a type-safe way.
 */
export const ACME_DATABASE_SCALARS = {
  acmeAccountKey: scalar().name("acme.account_key").type("TEXT"),
  acmeAccountUrl: scalar().name("acme.account_url").type("TEXT"),
} as const satisfies NamespacedScalars<"acme">
