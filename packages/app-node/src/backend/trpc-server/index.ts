import { createActor } from "@dassie/lib-reactive"

import { DassieActorContext } from "../base/types/dassie-base"
import { RegisterTrpcHttpUpgradeActor } from "./trpc-server"

export const TrpcServerActor = () =>
  createActor((sig: DassieActorContext) => {
    sig.run(RegisterTrpcHttpUpgradeActor)
  })
