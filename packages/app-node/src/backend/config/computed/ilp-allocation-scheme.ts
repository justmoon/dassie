import { type Reactor, createComputed } from "@dassie/lib-reactive"

import { DatabaseConfigStore } from "../database-config"

export type IlpAllocationScheme = "test" | "g"

export const IlpAllocationSchemeSignal = (reactor: Reactor) =>
  createComputed(reactor, (sig): IlpAllocationScheme => {
    const config = sig.readAndTrack(DatabaseConfigStore)

    return config.realm === "test" ? "test" : "g"
  })
