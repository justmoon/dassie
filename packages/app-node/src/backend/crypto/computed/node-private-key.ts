import { assert } from "@dassie/lib-logger"
import { type Reactor, createComputed } from "@dassie/lib-reactive"

import {
  DatabaseConfigStore,
  hasNodeIdentity,
} from "../../config/database-config"
import { crypto as logger } from "../../logger/instances"
import { parseEd25519Key } from "../../utils/pem"

export const NodePrivateKeySignal = (reactor: Reactor) =>
  createComputed(reactor, (sig) => {
    const config = sig.readAndTrack(DatabaseConfigStore)

    assert(logger, hasNodeIdentity(config), "node identity is not configured")

    return parseEd25519Key(config.dassieKey, "private")
  })
