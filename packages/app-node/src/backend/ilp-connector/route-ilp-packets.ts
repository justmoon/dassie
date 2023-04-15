import { createLogger } from "@dassie/lib-logger"
import { createActor, createSignal } from "@dassie/lib-reactive"

import { type IlpPreparePacket, IlpType } from "./ilp-packet-codec"
import { incomingIlpPacketTopic } from "./topics/incoming-ilp-packet"
import { outgoingIlpPacketBuffer } from "./topics/outgoing-ilp-packet"

const logger = createLogger("das:node:route-ilp-packets")

interface RequestIdMapEntry {
  sourceAddress: string
  sourceRequestId: number
  preparePacket: IlpPreparePacket & { type: typeof IlpType.Prepare }
}

export const requestIdMapSignal = () =>
  createSignal(new Map<number, RequestIdMapEntry>())

export const routeIlpPackets = () =>
  createActor((sig) => {
    const requestIdMap = sig.get(requestIdMapSignal)
    sig.on(incomingIlpPacketTopic, ({ source, packet, requestId }) => {
      const outgoingIlpPacketBufferInstance = sig.use(outgoingIlpPacketBuffer)
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

          sig.use(outgoingIlpPacketBuffer).emitEvent({
            source,
            packet,
            incomingRequestId: requestId,
            outgoingRequestId: outgoingRequestId,
          })
          return
        }
        case IlpType.Fulfill: {
          logger.debug("received ILP fulfill", {
            requestId,
          })

          const preparedEvent = outgoingIlpPacketBufferInstance.prepareEvent({
            source,
            packet,
            incomingRequestId: requestId,
          })

          logger.debug("sending ILP fulfill to source", {
            destination: preparedEvent.destination,
          })

          outgoingIlpPacketBufferInstance.emit(preparedEvent)

          return
        }
        case IlpType.Reject: {
          logger.debug("received ILP reject", {
            triggeredBy: packet.triggeredBy,
            message: packet.message,
            requestId,
          })

          const preparedEvent = outgoingIlpPacketBufferInstance.prepareEvent({
            source,
            packet,
            incomingRequestId: requestId,
          })

          logger.debug("sending ILP reject to source", {
            destination: preparedEvent.destination,
          })

          outgoingIlpPacketBufferInstance.emit(preparedEvent)

          return
        }
      }
    })
  })
