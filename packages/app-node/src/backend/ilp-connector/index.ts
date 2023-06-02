import { createActor } from "@dassie/lib-reactive"

import { processIncomingPacket } from "./process-incoming-packet"
import { sendOutgoingPackets } from "./send-outgoing-packets"

export const startIlpConnector = () =>
  createActor((sig) => {
    sig.run(processIncomingPacket)
    sig.run(sendOutgoingPackets)
  })
