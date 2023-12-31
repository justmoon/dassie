import { createActor } from "@dassie/lib-reactive"

import { HasTlsSignal } from "../config/computed/has-tls"
import { ServeFrontendActor } from "./serve-frontend"
import { ServeHttpActor } from "./serve-http"
import { ServeHttpsActor } from "./serve-https"

export const HttpServerActor = () =>
  createActor((sig) => {
    const hasTls = sig.readAndTrack(HasTlsSignal)
    sig.run(ServeHttpActor)

    if (hasTls) {
      sig.run(ServeFrontendActor)
      sig.run(ServeHttpsActor)
    }
  })
