import { createComputed } from "@dassie/lib-reactive"

import { databaseConfigPlain } from "../database-config"

export const ilpAllocationSchemeSignal = () =>
  createComputed((sig) =>
    sig.get(sig.use(databaseConfigPlain).realm) === "test" ? "test" : "g"
  )
