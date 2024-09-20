import { createActor } from "@dassie/lib-reactive"

import type { DassieActorContext } from "../base/types/dassie-base"
import { HasTlsSignal } from "../config/computed/has-tls"
import { RedirectToHttpsActor } from "./redirect-to-https"
import { ServeFrontendActor } from "./serve-frontend"

export const HttpServerActor = () =>
  createActor((sig: DassieActorContext) => {
    const hasTls = sig.readAndTrack(HasTlsSignal)
    sig.run(RedirectToHttpsActor)

    if (hasTls) {
      sig.run(ServeFrontendActor)
    }
  })
