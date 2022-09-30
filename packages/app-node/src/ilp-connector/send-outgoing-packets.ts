import { createLogger } from "@dassie/lib-logger"
import type { EffectContext } from "@dassie/lib-reactive"

import { ilpClientMapStore } from "./stores/ilp-client-map"
import { outgoingIlpPacketBuffer } from "./topics/outgoing-ilp-packet"

const logger = createLogger("das:node:send-outgoing-packets")

export const sendOutgoingPackets = (sig: EffectContext) => {
  const ilpClientMap = sig.use(ilpClientMapStore)

  sig.use(outgoingIlpPacketBuffer).on((event) => {
    const client = ilpClientMap.read().get(event.destination)
    if (!client) {
      throw new Error(`No client found for ${event.destination}`)
    }

    Promise.resolve(client.sendPacket(event)).catch((error: unknown) => {
      logger.error("failed to send outgoing packet", {
        error,
        to: event.destination,
      })
    })
  })
}
