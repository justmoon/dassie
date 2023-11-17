import { Reactor, createComputed } from "@dassie/lib-reactive"

import { DatabaseConfigStore } from "../database-config"

export const IlpAllocationSchemeSignal = (reactor: Reactor) =>
  createComputed(reactor, (sig) => {
    const config = sig.get(reactor.use(DatabaseConfigStore))

    return config.realm === "test" ? "test" : "g"
  })
