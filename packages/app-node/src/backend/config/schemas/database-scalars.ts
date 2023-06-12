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
  "config.tls_web_cert": {
    type: "TEXT",
    schema: z.string(),
  },
  "config.tls_web_key": {
    type: "TEXT",
    schema: z.string(),
  },
  "config.tls_dassie_cert": {
    type: "TEXT",
    schema: z.string(),
  },
  "config.tls_dassie_key": {
    type: "TEXT",
    schema: z.string(),
  },
} as const satisfies Record<`config.${string}`, ScalarDescription>
