import { createActor } from "@dassie/lib-reactive"

import { serveLocalIpc } from "./actors/serve-local-ipc"

export const startLocalRpcServer = () =>
  createActor((sig) => {
    sig.run(serveLocalIpc)
  })
