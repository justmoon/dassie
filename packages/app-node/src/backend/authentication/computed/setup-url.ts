import { Reactor, createComputed } from "@dassie/lib-reactive"

import { DatabaseConfigStore } from "../../config/database-config"
import { SetupAuthorizationTokenSignal } from "../signals/setup-authorization-token"

export const SetupUrlSignal = (reactor: Reactor) =>
  createComputed(reactor, (sig) => {
    const token = sig.readAndTrack(SetupAuthorizationTokenSignal)
    const hostname = sig.readAndTrack(DatabaseConfigStore).hostname

    return `https://${hostname}/setup/${token}`
  })
