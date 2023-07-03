import assert from "node:assert"

import { createComputed } from "@dassie/lib-reactive"

import { databaseConfigPlain } from "../../config/database-config"
import { parseEd25519PrivateKey } from "../../utils/pem"

export const nodePrivateKeySignal = () =>
  createComputed((sig) => {
    const config = sig.use(databaseConfigPlain)

    assert(config.hasNodeIdentity, "Node identity is not configured")

    return parseEd25519PrivateKey(sig.get(config.tlsDassieKey))
  })
