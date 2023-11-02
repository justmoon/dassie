import {
  BtpType,
  btpEnvelopeSchema,
  btpMessageSchema,
} from "@dassie/lib-protocol-utils"
import { Reactor } from "@dassie/lib-reactive"

import { RegisterBtpHttpUpgradeActor } from "../../btp-server/register-btp-http-upgrade"
import { connector as logger } from "../../logger/instances"
import { CommonEndpointInfo, PacketSender } from "../functions/send-packet"

export interface BtpEndpointInfo extends CommonEndpointInfo {
  readonly type: "btp"
  readonly connectionId: number
}

export const SendBtpPackets = (reactor: Reactor): PacketSender<"btp"> => {
  const btpBroker = reactor.use(RegisterBtpHttpUpgradeActor)

  return {
    sendPrepare: ({
      parsedPacket,
      serializedPacket,
      outgoingRequestId: requestId,
      connectionId,
    }) => {
      if (parsedPacket.amount === 0n) {
        const btpMessageSerializeResult = btpMessageSchema.serialize({
          protocolData: [
            {
              protocolName: "ilp",
              contentType: 0,
              data: serializedPacket,
            },
          ],
        })

        if (!btpMessageSerializeResult.success) {
          logger.error("could not serialize BTP message", {
            error: btpMessageSerializeResult.error,
          })
          return
        }

        const btpEnvelopeSerializeResult = btpEnvelopeSchema.serialize({
          messageType: BtpType.Message,
          requestId,
          message: btpMessageSerializeResult.value,
        })

        if (!btpEnvelopeSerializeResult.success) {
          logger.error("could not serialize BTP envelope", {
            error: btpEnvelopeSerializeResult.error,
          })
          return
        }

        btpBroker.api.send.tell({
          connectionId,
          message: btpEnvelopeSerializeResult.value,
        })

        return
      }

      const btpMessageSerializeResult = btpMessageSchema.serialize({
        protocolData: [
          {
            protocolName: "ilp",
            contentType: 0,
            data: serializedPacket,
          },
        ],
      })

      if (!btpMessageSerializeResult.success) {
        logger.error("could not serialize BTP message", {
          error: btpMessageSerializeResult.error,
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
          error: btpEnvelopeSerializeResult.error,
        })
        return
      }

      btpBroker.api.send.tell({
        connectionId,
        message: btpEnvelopeSerializeResult.value,
      })

      return
    },

    sendResult: ({
      serializedPacket,
      prepare: { incomingRequestId: requestId },
      connectionId,
    }) => {
      const btpMessageSerializeResult = btpMessageSchema.serialize({
        protocolData: [
          {
            protocolName: "ilp",
            contentType: 0,
            data: serializedPacket,
          },
        ],
      })

      if (!btpMessageSerializeResult.success) {
        logger.error("could not serialize BTP message", {
          error: btpMessageSerializeResult.error,
        })
        return
      }

      const btpEnvelopeSerializeResult = btpEnvelopeSchema.serialize({
        messageType: BtpType.Response,
        requestId,
        message: btpMessageSerializeResult.value,
      })

      if (!btpEnvelopeSerializeResult.success) {
        logger.error("could not serialize BTP envelope", {
          error: btpEnvelopeSerializeResult.error,
        })
        return
      }

      btpBroker.api.send.tell({
        connectionId,
        message: btpEnvelopeSerializeResult.value,
      })
    },
  }
}
