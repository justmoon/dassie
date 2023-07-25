import { createActor } from "@dassie/lib-reactive"

import { serveTokens } from "./serve-tokens"

export const startAcmeCertificateManager = () =>
  createActor((sig) => {
    sig.run(serveTokens)
  })
