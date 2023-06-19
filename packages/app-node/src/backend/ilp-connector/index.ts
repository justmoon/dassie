import { createActor } from "@dassie/lib-reactive"

import { processPacket } from "./process-packet"

export const startIlpConnector = () =>
  createActor((sig) => {
    sig.run(processPacket)
  })
