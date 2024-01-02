import { createActor } from "@dassie/lib-reactive"

import { DassieActorContext } from "../base/types/dassie-base"
import { ServeLocalIpcActor } from "./actors/serve-local-ipc"

export const LocalRpcServerActor = () =>
  createActor((sig: DassieActorContext) => {
    sig.run(ServeLocalIpcActor)
  })
