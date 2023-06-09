import { createLogger } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

import { EMPTY_UINT8ARRAY } from "../../../common/constants/general"
import { ilpAllocationSchemeSignal } from "../../config/computed/ilp-allocation-scheme"
import { processIncomingPacket } from "../../ilp-connector/process-incoming-packet"
import type { IncomingPeerMessageEvent } from "../actors/handle-peer-message"

const logger = createLogger("das:node:handle-interledger-packet")

export const handleInterledgerPacket = () =>
  createActor((sig) => {
    const ilpAllocationScheme = sig.get(ilpAllocationSchemeSignal)
    const processIncomingPacketActor = sig.use(processIncomingPacket)

    return {
      handle: ({
        message: {
          sender,
          content: {
            value: { value: content },
          },
        },
        authenticated,
        peerState,
      }: IncomingPeerMessageEvent<"interledgerPacket">) => {
        if (!authenticated) {
          logger.warn("received unauthenticated interledger packet, discarding")
          return EMPTY_UINT8ARRAY
        }

        if (peerState?.id !== "peered") {
          logger.warn(
            "received interledger packet from unpeered node, discarding"
          )
          return EMPTY_UINT8ARRAY
        }

        const { subnetId } = peerState

        processIncomingPacketActor.tell("handle", {
          sourceIlpAddress: `${ilpAllocationScheme}.das.${sender}`,
          ledgerAccountPath: `${subnetId}/peer/${sender}/interledger`,
          serializedPacket: content.signed.packet,
          requestId: content.signed.requestId,
        })

        return EMPTY_UINT8ARRAY
      },
    }
  })
