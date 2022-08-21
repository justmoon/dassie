import { createLogger } from "@dassie/lib-logger"
import type { EffectContext } from "@dassie/lib-reactive"

import { configStore } from "../config"
import { ildcpResponseSchema } from "./ildcp-packet-codec"
import {
  IlpType,
  ilpEnvelopeSchema,
  ilpFulfillSchema,
  ilpPrepareSchema,
  ilpRejectSchema,
} from "./ilp-packet-codec"
import { incomingIlpPacketBuffer } from "./topics/incoming-ilp-packet"
import { outgoingIlpPacketBuffer } from "./topics/outgoing-ilp-packet"

const logger = createLogger("das:node:route-ilp-packets")

interface RequestIdMapEntry {
  sourceAddress: string
  sourceRequestId: number
}

const requestIdMap = new Map<number, RequestIdMapEntry>()

export const routeIlpPackets = (sig: EffectContext) => {
  const ilpAddress = sig.get(configStore, (config) => config.ilpAddress)

  sig.on(incomingIlpPacketBuffer, ({ source, packet, requestId }) => {
    const envelopeParseResult = ilpEnvelopeSchema.parse(packet)

    if (!envelopeParseResult.success) {
      logger.debug("failed to parse ILP envelope", {
        error: envelopeParseResult.failure,
      })
      return
    }

    switch (envelopeParseResult.value.packetType) {
      case IlpType.Prepare: {
        const prepareParseResult = ilpPrepareSchema.parse(
          envelopeParseResult.value.data
        )

        if (!prepareParseResult.success) {
          logger.debug("failed to parse ILP prepare", {
            error: prepareParseResult.failure,
          })
          return
        }

        logger.debug("received ILP prepare", {
          destination: prepareParseResult.value.destination,
          amount: prepareParseResult.value.amount,
        })

        if (prepareParseResult.value.destination === "peer.config") {
          // IL-DCP
          const ildcpSerializationResult = ildcpResponseSchema.serialize({
            address: source,
            assetScale: 9,
            assetCode: "XRP",
          })

          if (!ildcpSerializationResult.success) {
            logger.debug("failed to serialize IL-DCP response", {
              error: ildcpSerializationResult.failure,
            })
            return
          }

          const fulfillSerializationResult = ilpFulfillSchema.serialize({
            fulfillment: new Uint8Array(32),
            data: ildcpSerializationResult.value,
          })

          if (!fulfillSerializationResult.success) {
            logger.debug("failed to serialize ILP fulfill", {
              error: fulfillSerializationResult.failure,
            })
            return
          }

          const envelopeSerializationResult = ilpEnvelopeSchema.serialize({
            packetType: 13,
            data: fulfillSerializationResult.value,
          })

          if (!envelopeSerializationResult.success) {
            logger.debug("failed to serialize ILP envelope", {
              error: envelopeSerializationResult.failure,
            })
            return
          }

          logger.debug("sending IL-DCP response", {
            destination: source,
          })

          sig.emit(outgoingIlpPacketBuffer, {
            destination: source,
            packet: envelopeSerializationResult.value,
            amount: 0n,
            requestId,
            isResponse: true,
          })
          return
        } else if (
          prepareParseResult.value.destination.startsWith(`${ilpAddress}.c`)
        ) {
          const localDestination = prepareParseResult.value.destination
            .split(".")
            .slice(0, 5)
            .join(".")

          const outgoingRequestId = Math.trunc(Math.random() * 0xff_ff_ff_ff)

          requestIdMap.set(outgoingRequestId, {
            sourceAddress: source,
            sourceRequestId: requestId,
          })

          logger.debug("forwarding ILP prepare", {
            source,
            localDestination,
            destination: prepareParseResult.value.destination,
          })

          sig.emit(outgoingIlpPacketBuffer, {
            destination: localDestination,
            packet,
            amount: prepareParseResult.value.amount,
            requestId: outgoingRequestId,
            isResponse: false,
          })
          return
        }
        return
      }
      case IlpType.Fulfill: {
        const fulfillParseResult = ilpFulfillSchema.parse(
          envelopeParseResult.value.data
        )

        if (!fulfillParseResult.success) {
          logger.debug("failed to parse ILP fulfill", {
            error: fulfillParseResult.failure,
          })
          return
        }

        logger.debug("received ILP fulfill")

        const mappedRequest = requestIdMap.get(requestId)

        if (mappedRequest) {
          logger.debug("sending ILP fulfill to source", {
            destination: mappedRequest.sourceAddress,
          })
          sig.emit(outgoingIlpPacketBuffer, {
            destination: mappedRequest.sourceAddress,
            packet,
            amount: 0n,
            requestId: mappedRequest.sourceRequestId,
            isResponse: true,
          })
        }

        return
      }
      case IlpType.Reject: {
        const rejectParseResult = ilpRejectSchema.parse(
          envelopeParseResult.value.data
        )

        if (!rejectParseResult.success) {
          logger.debug("failed to parse ILP reject", {
            error: rejectParseResult.failure,
          })
          return
        }

        logger.debug("received ILP reject", {
          triggeredBy: rejectParseResult.value.triggeredBy,
          message: rejectParseResult.value.message,
        })

        const mappedRequest = requestIdMap.get(requestId)

        if (mappedRequest) {
          logger.debug("sending ILP reject to source", {
            destination: mappedRequest.sourceAddress,
          })
          sig.emit(outgoingIlpPacketBuffer, {
            destination: mappedRequest.sourceAddress,
            packet,
            amount: 0n,
            requestId: mappedRequest.sourceRequestId,
            isResponse: true,
          })
        }

        return
      }
    }
  })
}
