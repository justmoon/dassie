import { createActor } from "@dassie/lib-reactive"

import { ServeTokensActor } from "./serve-tokens"

export const AcmeCertificateManagerActor = () =>
  createActor((sig) => {
    sig.run(ServeTokensActor)
  })
