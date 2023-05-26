import { createActor } from "@dassie/lib-reactive"

import { handleIldcpRequests } from "./handle-ildcp-requests"

export const startIldcpServer = () =>
  createActor((sig) => {
    sig.run(handleIldcpRequests, undefined, { register: true })
  })
