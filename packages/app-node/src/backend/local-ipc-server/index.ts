import { createActor } from "@dassie/lib-reactive"

import { ServeLocalIpcActor } from "./actors/serve-local-ipc"

export const LocalRpcServerActor = () =>
  createActor((sig) => {
    sig.run(ServeLocalIpcActor)
  })
