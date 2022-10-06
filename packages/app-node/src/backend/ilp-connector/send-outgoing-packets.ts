import { createLogger } from "@dassie/lib-logger"
import type { EffectContext } from "@dassie/lib-reactive"

import { ilpRoutingTableSignal } from "./signals/ilp-routing-table"
import { outgoingIlpPacketBuffer } from "./topics/outgoing-ilp-packet"

const logger = createLogger("das:node:send-outgoing-packets")

export const sendOutgoingPackets = (sig: EffectContext) => {
  const ilpClientMap = sig.use(ilpRoutingTableSignal)

  sig.use(outgoingIlpPacketBuffer).on((event) => {
    const client = ilpClientMap.read().lookup(event.destination)
    if (!client) {
      throw new Error(`No routing table entry found for ${event.destination}`)
    }

    Promise.resolve(client.sendPacket(event)).catch((error: unknown) => {
      logger.error("failed to send outgoing packet", {
        error,
        to: event.destination,
      })
    })
  })
}
