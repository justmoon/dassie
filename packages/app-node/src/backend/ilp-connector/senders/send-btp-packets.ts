import {
  BtpType,
  btpEnvelopeSchema,
  btpMessageSchema,
} from "@dassie/lib-protocol-utils"
import { isFailure } from "@dassie/lib-type-utils"

import { DassieReactor } from "../../base/types/dassie-base"
import { RegisterBtpHttpUpgradeActor } from "../../btp-server/register-btp-http-upgrade"
import { connector as logger } from "../../logger/instances"
import { CommonEndpointInfo, PacketSender } from "../functions/send-packet"

export interface BtpEndpointInfo extends CommonEndpointInfo {
  readonly type: "btp"
  readonly connectionId: number
}

export const SendBtpPackets = (
  reactor: DassieReactor,
): PacketSender<"btp"> => ({
  sendPrepare: ({
    parsedPacket,
    serializedPacket,
    outgoingRequestId: requestId,
    destinationEndpointInfo: { connectionId },
  }) => {
    const btpBroker = reactor.use(RegisterBtpHttpUpgradeActor)

    if (parsedPacket.data.amount === 0n) {
      const btpMessageSerializeResult = btpMessageSchema.serialize({
        protocolData: [
          {
            protocolName: "ilp",
            contentType: 0,
            data: serializedPacket,
          },
        ],
      })

      if (isFailure(btpMessageSerializeResult)) {
        logger.error("could not serialize BTP message", {
          error: btpMessageSerializeResult,
        })
        return
      }

      const btpEnvelopeSerializeResult = btpEnvelopeSchema.serialize({
        messageType: BtpType.Message,
        requestId,
        message: btpMessageSerializeResult,
      })

      if (isFailure(btpEnvelopeSerializeResult)) {
        logger.error("could not serialize BTP envelope", {
          error: btpEnvelopeSerializeResult,
        })
        return
      }

      btpBroker.api.send.tell({
        connectionId,
        message: btpEnvelopeSerializeResult,
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

    if (isFailure(btpMessageSerializeResult)) {
      logger.error("could not serialize BTP message", {
        error: btpMessageSerializeResult,
      })
      return
    }

    const btpEnvelopeSerializeResult = btpEnvelopeSchema.serialize({
      messageType: 6, // Message
      requestId,
      message: btpMessageSerializeResult,
    })

    if (isFailure(btpEnvelopeSerializeResult)) {
      logger.error("could not serialize BTP envelope", {
        error: btpEnvelopeSerializeResult,
      })
      return
    }

    btpBroker.api.send.tell({
      connectionId,
      message: btpEnvelopeSerializeResult,
    })

    return
  },

  sendResult: ({
    serializedPacket,
    prepare: { incomingRequestId: requestId },
    destinationEndpointInfo: { connectionId },
  }) => {
    const btpBroker = reactor.use(RegisterBtpHttpUpgradeActor)
    const btpMessageSerializeResult = btpMessageSchema.serialize({
      protocolData: [
        {
          protocolName: "ilp",
          contentType: 0,
          data: serializedPacket,
        },
      ],
    })

    if (isFailure(btpMessageSerializeResult)) {
      logger.error("could not serialize BTP message", {
        error: btpMessageSerializeResult,
      })
      return
    }

    logger.assert(
      typeof requestId === "number",
      "expected request id to be a number for BTP",
    )

    const btpEnvelopeSerializeResult = btpEnvelopeSchema.serialize({
      messageType: BtpType.Response,
      requestId,
      message: btpMessageSerializeResult,
    })

    if (isFailure(btpEnvelopeSerializeResult)) {
      logger.error("could not serialize BTP envelope", {
        error: btpEnvelopeSerializeResult,
      })
      return
    }

    btpBroker.api.send.tell({
      connectionId,
      message: btpEnvelopeSerializeResult,
    })
  },
})
