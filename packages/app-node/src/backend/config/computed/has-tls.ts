import { Reactor, createComputed } from "@dassie/lib-reactive"

import { DatabaseConfigStore, hasTls } from "../database-config"

export const HasTlsSignal = (reactor: Reactor) =>
  createComputed(reactor, (sig) => {
    const config = sig.get(reactor.use(DatabaseConfigStore))
    return hasTls(config)
  })
