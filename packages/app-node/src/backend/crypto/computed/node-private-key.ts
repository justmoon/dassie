import assert from "node:assert"

import { Reactor, createComputed } from "@dassie/lib-reactive"

import {
  DatabaseConfigStore,
  hasNodeIdentity,
} from "../../config/database-config"
import { parseEd25519PrivateKey } from "../../utils/pem"

export const NodePrivateKeySignal = (reactor: Reactor) =>
  createComputed(reactor, (sig) => {
    const config = sig.readAndTrack(DatabaseConfigStore)

    assert(hasNodeIdentity(config), "Node identity is not configured")

    return parseEd25519PrivateKey(config.dassieKey)
  })
