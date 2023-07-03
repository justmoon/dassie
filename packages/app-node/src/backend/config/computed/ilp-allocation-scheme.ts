import assert from "node:assert"

import { createComputed } from "@dassie/lib-reactive"

import { databaseConfigPlain } from "../database-config"

export const ilpAllocationSchemeSignal = () =>
  createComputed((sig) => {
    const config = sig.use(databaseConfigPlain)

    assert(config.hasWebUi, "Web UI is not configured")

    return sig.get(config.realm) === "test" ? "test" : "g"
  })
