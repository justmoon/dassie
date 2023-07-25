import { createComputed } from "@dassie/lib-reactive"

import { databaseConfigStore } from "../database-config"

export const ilpAllocationSchemeSignal = () =>
  createComputed((sig) => {
    const config = sig.get(databaseConfigStore)

    return config.realm === "test" ? "test" : "g"
  })
