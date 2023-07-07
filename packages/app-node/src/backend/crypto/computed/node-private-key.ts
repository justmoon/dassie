import assert from "node:assert"

import { createComputed } from "@dassie/lib-reactive"

import { databaseConfigSignal } from "../../config/database-config"
import { parseEd25519PrivateKey } from "../../utils/pem"

export const nodePrivateKeySignal = () =>
  createComputed((sig) => {
    const config = sig.get(databaseConfigSignal)

    assert(config.hasNodeIdentity, "Node identity is not configured")

    return parseEd25519PrivateKey(config.tlsDassieKey)
  })
