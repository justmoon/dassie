import { z } from "zod"

import { type ScalarDescription } from "@dassie/lib-sqlite"

import { VALID_REALMS } from "../../constants/general"

/**
 * This defines all the scalar values that are stored in the database which will
 * allow us to access them in a type-safe way.
 */
export const CONFIG_DATABASE_SCALARS = {
  "config.realm": {
    type: "TEXT",
    schema: z.enum(VALID_REALMS),
  },
} as const satisfies Record<`config.${string}`, ScalarDescription>
