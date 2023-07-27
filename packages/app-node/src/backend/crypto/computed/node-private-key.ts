import assert from "node:assert"

import { createComputed } from "@dassie/lib-reactive"

import {
  databaseConfigStore,
  hasNodeIdentity,
} from "../../config/database-config"
import { parseEd25519PrivateKey } from "../../utils/pem"

export const nodePrivateKeySignal = () =>
  createComputed((sig) => {
    const config = sig.get(databaseConfigStore)

    assert(hasNodeIdentity(config), "Node identity is not configured")

    return parseEd25519PrivateKey(config.dassieKey)
  })
