import { createActor } from "@dassie/lib-reactive"

import { RegisterTrpcHttpUpgradeActor } from "./trpc-server"

export const TrpcServerActor = () =>
  createActor((sig) => {
    sig.run(RegisterTrpcHttpUpgradeActor)
  })
