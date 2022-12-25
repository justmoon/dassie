import type { EffectContext } from "@dassie/lib-reactive"

import { ilpRoutingTableSignal } from "./signals/ilp-routing-table"
import { outgoingIlpPacketBuffer } from "./topics/outgoing-ilp-packet"

export const sendOutgoingPackets = (sig: EffectContext) => {
  const ilpClientMap = sig.use(ilpRoutingTableSignal)

  sig.onAsync(outgoingIlpPacketBuffer, async (event) => {
    const client = ilpClientMap.read().lookup(event.destination)
    if (!client) {
      throw new Error(`No routing table entry found for ${event.destination}`)
    }

    await client.sendPacket(event)
  })
}
