import { Reactor, createTopic } from "@dassie/lib-reactive"

import { IlpPacket, IlpType, serializeIlpPacket } from "../ilp-packet-codec"
import { requestIdMapSignal } from "../route-ilp-packets"
import type { IlpPacketWithAttachedPrepare } from "./incoming-ilp-packet"

export interface OutgoingIlpPacket {
  destination: string
  source: string
  packet: IlpPacketWithAttachedPrepare
  asUint8Array: Uint8Array
  requestId: number
}

interface OutgoingPacketParameters {
  source: string
  packet: IlpPacket
  outgoingRequestId?: number
  incomingRequestId: number
}

export const outgoingIlpPacketBuffer = (reactor: Reactor) => {
  const topic = createTopic<OutgoingIlpPacket>()
  const requestIdMap = reactor.use(requestIdMapSignal).read()

  const prepareEvent = (
    outgoingPacketParameters: OutgoingPacketParameters
  ): OutgoingIlpPacket => {
    const { packet, incomingRequestId } = outgoingPacketParameters
    const serializedPacket = serializeIlpPacket(packet)

    if (packet.type === IlpType.Prepare) {
      const outgoingRequestId =
        outgoingPacketParameters.outgoingRequestId ??
        Math.trunc(Math.random() * 0xff_ff_ff_ff)

      return {
        ...outgoingPacketParameters,
        packet,
        asUint8Array: serializedPacket,
        requestId: outgoingRequestId,
        destination: packet.destination,
      }
    } else {
      const requestMapEntry = requestIdMap.get(incomingRequestId)

      if (!requestMapEntry) {
        throw new Error(
          "Received response ILP packet which did not match any request ILP packet we sent"
        )
      }

      requestIdMap.delete(incomingRequestId)

      return {
        ...outgoingPacketParameters,
        packet: {
          ...packet,
          prepare: requestMapEntry.preparePacket,
        },
        asUint8Array: serializedPacket,

        requestId: requestMapEntry.sourceRequestId,
        destination: requestMapEntry.sourceAddress,
      }
    }
  }

  return {
    ...topic,
    prepareEvent,
    emitEvent: (outgoingPacketParameters: OutgoingPacketParameters) => {
      topic.emit(prepareEvent(outgoingPacketParameters))
    },
  }
}
