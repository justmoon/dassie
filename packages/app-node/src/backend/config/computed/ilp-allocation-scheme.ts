import assert from "node:assert"

import { createComputed } from "@dassie/lib-reactive"

import { databaseConfigSignal } from "../database-config"

export const ilpAllocationSchemeSignal = () =>
  createComputed((sig) => {
    const config = sig.get(databaseConfigSignal)

    assert(config.hasWebUi, "Web UI is not configured")

    return config.realm === "test" ? "test" : "g"
  })
