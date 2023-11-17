import { Reactor, createComputed } from "@dassie/lib-reactive"

import { DatabaseConfigStore, hasNodeIdentity } from "../database-config"

export const HasNodeIdentitySignal = (reactor: Reactor) =>
  createComputed(reactor, (sig) => {
    const config = sig.get(DatabaseConfigStore)
    return hasNodeIdentity(config)
  })
