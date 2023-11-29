import { Reactor, createComputed } from "@dassie/lib-reactive"

import {
  DatabaseConfigStore,
  hasNodeIdentity,
} from "../../config/database-config"
import { crypto as logger } from "../../logger/instances"
import { parseEd25519PrivateKey } from "../../utils/pem"

export const NodePrivateKeySignal = (reactor: Reactor) =>
  createComputed(reactor, (sig) => {
    const config = sig.readAndTrack(DatabaseConfigStore)

    logger.assert(hasNodeIdentity(config), "node identity is not configured")

    return parseEd25519PrivateKey(config.dassieKey)
  })
