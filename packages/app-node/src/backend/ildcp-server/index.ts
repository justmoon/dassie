import { createActor } from "@dassie/lib-reactive"

import { HandleIldcpRequestsActor } from "./handle-ildcp-requests"

export const IldcpServerActor = () =>
  createActor((sig) => {
    sig.run(HandleIldcpRequestsActor)
  })
