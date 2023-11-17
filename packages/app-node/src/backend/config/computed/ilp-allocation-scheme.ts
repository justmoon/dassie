import { Reactor, createComputed } from "@dassie/lib-reactive"

import { DatabaseConfigStore } from "../database-config"

export const IlpAllocationSchemeSignal = (reactor: Reactor) =>
  createComputed(reactor, (sig) => {
    const config = sig.readAndTrack(DatabaseConfigStore)

    return config.realm === "test" ? "test" : "g"
  })
