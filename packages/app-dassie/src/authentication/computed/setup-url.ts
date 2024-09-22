import { createComputed } from "@dassie/lib-reactive"

import type { DassieReactor } from "../../base/types/dassie-base"
import { HasTlsSignal } from "../../config/computed/has-tls"
import { DatabaseConfigStore } from "../../config/database-config"
import { SetupAuthorizationTokenSignal } from "../signals/setup-authorization-token"

export const SetupUrlSignal = (reactor: DassieReactor) =>
  createComputed(reactor, (sig) => {
    const token = sig.readAndTrack(SetupAuthorizationTokenSignal)
    const hasTls = sig.readAndTrack(HasTlsSignal)

    if (hasTls) {
      const hostname = sig.readAndTrack(DatabaseConfigStore).hostname

      return `https://${hostname}/setup/${token}`
    } else {
      return `http://127.0.0.1/setup/${token}`
    }
  })
