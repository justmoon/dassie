import { createActor } from "@dassie/lib-reactive"

import { registerTrpcHttpUpgrade } from "./trpc-server"

export const startTrpcServer = () =>
  createActor((sig) => {
    sig.run(registerTrpcHttpUpgrade)
  })
