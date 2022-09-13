import { createLogger } from "@dassie/lib-logger"
import { btpEnvelopeSchema, btpMessageSchema } from "@dassie/lib-protocol-utils"
import type { EffectContext } from "@dassie/lib-reactive"

import { outgoingIlpPacketBuffer } from "../ilp-connector/topics/outgoing-ilp-packet"
import { connectionMap } from "./register-btp-http-upgrade"

const logger = createLogger("das:node:btp-ilp-sender")

export const registerBtpIlpSender = (sig: EffectContext) => {
  sig.on(
    outgoingIlpPacketBuffer,
    ({ destination, packet, amount, requestId, isResponse }) => {
      const socket = connectionMap.get(destination)

      if (!socket) {
        logger.error("could not forward ILP packet, BTP socket not found", {
          destination,
        })
        return
      }

      if (amount === 0n) {
        const btpMessageSerializeResult = btpMessageSchema.serialize({
          protocolData: [
            {
              protocolName: "ilp",
              contentType: 0,
              data: packet,
            },
          ],
        })

        if (!btpMessageSerializeResult.success) {
          logger.error("could not serialize BTP message", {
            error: btpMessageSerializeResult.failure,
          })
          return
        }

        const btpEnvelopeSerializeResult = btpEnvelopeSchema.serialize({
          messageType: isResponse ? 1 : 6, // Response or Message
          requestId,
          message: btpMessageSerializeResult.value,
        })

        if (!btpEnvelopeSerializeResult.success) {
          logger.error("could not serialize BTP envelope", {
            error: btpEnvelopeSerializeResult.failure,
          })
          return
        }

        socket.send(btpEnvelopeSerializeResult.value)

        return
      } else {
        const btpMessageSerializeResult = btpMessageSchema.serialize({
          protocolData: [
            {
              protocolName: "ilp",
              contentType: 0,
              data: packet,
            },
          ],
        })

        if (!btpMessageSerializeResult.success) {
          logger.error("could not serialize BTP message", {
            error: btpMessageSerializeResult.failure,
          })
          return
        }

        // const btpTransferSerializeResult = btpTransferSchema.serialize({
        //   amount,
        //   protocolData: [
        //     {
        //       protocolName: "ilp",
        //       contentType: 0,
        //       data: packet,
        //     },
        //   ],
        // })

        // if (!btpTransferSerializeResult.success) {
        //   logger.error("could not serialize BTP transfer", {
        //     error: btpTransferSerializeResult.failure,
        //   })
        //   return
        // }

        const btpEnvelopeSerializeResult = btpEnvelopeSchema.serialize({
          messageType: 6, // Message
          requestId,
          message: btpMessageSerializeResult.value,
        })

        if (!btpEnvelopeSerializeResult.success) {
          logger.error("could not serialize BTP envelope", {
            error: btpEnvelopeSerializeResult.failure,
          })
          return
        }

        socket.send(btpEnvelopeSerializeResult.value)

        return
      }
    }
  )
}
