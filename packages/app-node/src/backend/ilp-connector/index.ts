import { createActor } from "@dassie/lib-reactive"

import { ProcessPacketActor } from "./process-packet"

export const IlpConnectorActor = () =>
  createActor((sig) => {
    sig.run(ProcessPacketActor)
  })
