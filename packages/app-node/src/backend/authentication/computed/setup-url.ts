import { Reactor, createComputed } from "@dassie/lib-reactive"

import { DatabaseConfigStore } from "../../config/database-config"
import { SetupAuthorizationTokenSignal } from "../signals/setup-authorization-token"

export const SetupUrlSignal = (reactor: Reactor) =>
  createComputed(reactor.lifecycle, (sig) => {
    const token = sig.get(reactor.use(SetupAuthorizationTokenSignal))
    const hostname = sig.get(reactor.use(DatabaseConfigStore)).hostname

    return `https://${hostname}/setup/${token}`
  })
