import { type Reactor, createTopic } from "@dassie/lib-reactive"

import {
  type IlpFulfillPacket,
  type IlpPacket,
  type IlpPreparePacket,
  type IlpRejectPacket,
  IlpType,
  parseIlpPacket,
} from "../ilp-packet-codec"
import { requestIdMapSignal } from "../route-ilp-packets"

type WithAttachedPrepare<T extends IlpPacket> = T extends
  | IlpFulfillPacket
  | IlpRejectPacket
  ? T & { prepare: IlpPreparePacket }
  : T

export type IlpPacketWithAttachedPrepare = WithAttachedPrepare<IlpPacket>

export interface IncomingIlpPacket {
  source: string
  packet: IlpPacketWithAttachedPrepare
  asUint8Array: Uint8Array
  requestId: number
}

interface IncomingPacketParameters {
  source: string
  packet: Uint8Array
  requestId: number
}

export const incomingIlpPacketTopic = (reactor: Reactor) => {
  const topic = createTopic<IncomingIlpPacket>()
  const requestIdMap = reactor.use(requestIdMapSignal).read()

  const parseAndLookupPacket = (
    incomingPacketParameters: IncomingPacketParameters
  ): IlpPacketWithAttachedPrepare => {
    const parsedPacket = parseIlpPacket(incomingPacketParameters.packet)

    if (parsedPacket.type === IlpType.Prepare) {
      return parsedPacket
    } else {
      const requestMapEntry = requestIdMap.get(
        incomingPacketParameters.requestId
      )
      if (!requestMapEntry) {
        throw new Error(
          "Received response ILP packet which did not match any request ILP packet we sent"
        )
      }

      return {
        ...parsedPacket,
        prepare: requestMapEntry.preparePacket,
      }
    }
  }

  const prepareEvent = (incomingPacketParameters: IncomingPacketParameters) => {
    return {
      ...incomingPacketParameters,
      packet: parseAndLookupPacket(incomingPacketParameters),
      asUint8Array: incomingPacketParameters.packet,
    }
  }

  return {
    ...topic,
    prepareEvent,
    emitPacket: (incomingPacketParameters: IncomingPacketParameters) => {
      topic.emit(prepareEvent(incomingPacketParameters))
    },
  }
}
