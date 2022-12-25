import { createLogger } from "@dassie/lib-logger"
import type { EffectContext } from "@dassie/lib-reactive"

import { IlpType } from "./ilp-packet-codec"
import { incomingIlpPacketTopic } from "./topics/incoming-ilp-packet"
import { outgoingIlpPacketBuffer } from "./topics/outgoing-ilp-packet"

const logger = createLogger("das:node:route-ilp-packets")

interface RequestIdMapEntry {
  sourceAddress: string
  sourceRequestId: number
}

const requestIdMap = new Map<number, RequestIdMapEntry>()

export const routeIlpPackets = (sig: EffectContext) => {
  sig.on(
    incomingIlpPacketTopic,
    ({ source, packet, asUint8Array, requestId }) => {
      switch (packet.type) {
        case IlpType.Prepare: {
          logger.debug("received ILP prepare", {
            from: source,
            to: packet.destination,
            amount: packet.amount,
          })

          const outgoingRequestId = Math.trunc(Math.random() * 0xff_ff_ff_ff)

          requestIdMap.set(outgoingRequestId, {
            sourceAddress: source,
            sourceRequestId: requestId,
            preparePacket: packet,
          })

          logger.debug("forwarding ILP prepare", {
            source,
            destination: packet.destination,
            requestId: outgoingRequestId,
          })

          sig.use(outgoingIlpPacketBuffer).emit({
            destination: packet.destination,
            packet,
            asUint8Array,
            requestId: outgoingRequestId,
          })
          return
        }
        case IlpType.Fulfill: {
          logger.debug("received ILP fulfill", {
            requestId,
          })

          const mappedRequest = requestIdMap.get(requestId)

          if (mappedRequest) {
            logger.debug("sending ILP fulfill to source", {
              destination: mappedRequest.sourceAddress,
            })
            sig.use(outgoingIlpPacketBuffer).emit({
              destination: mappedRequest.sourceAddress,
              packet,
              asUint8Array,
              requestId: mappedRequest.sourceRequestId,
            })
          }

          return
        }
        case IlpType.Reject: {
          logger.debug("received ILP reject", {
            triggeredBy: packet.triggeredBy,
            message: packet.message,
            requestId,
          })

          const mappedRequest = requestIdMap.get(requestId)

          if (mappedRequest) {
            logger.debug("sending ILP reject to source", {
              destination: mappedRequest.sourceAddress,
            })
            sig.use(outgoingIlpPacketBuffer).emit({
              destination: mappedRequest.sourceAddress,
              packet,
              asUint8Array,
              requestId: mappedRequest.sourceRequestId,
            })
          }

          return
        }
      }
    }
  )
}
