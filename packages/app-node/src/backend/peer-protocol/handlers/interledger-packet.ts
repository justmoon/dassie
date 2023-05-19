import { createLogger } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

import { EMPTY_UINT8ARRAY } from "../../../common/constants/general"
import { configSignal } from "../../config"
import { processIncomingPacket } from "../../ilp-connector/process-incoming-packet"
import type { IncomingPeerMessageEvent } from "../actors/handle-peer-message"

const logger = createLogger("das:node:handle-interledger-packet")

export const handleInterledgerPacket = () =>
  createActor((sig) => {
    const { ilpAllocationScheme } = sig.getKeys(configSignal, [
      "ilpAllocationScheme",
    ])
    const processIncomingPacketActor = sig.use(processIncomingPacket)

    return {
      handle: ({
        message: {
          sender,
          subnetId,
          content: {
            value: { value: content },
          },
        },
        authenticated,
      }: IncomingPeerMessageEvent<"interledgerPacket">) => {
        if (!authenticated) {
          logger.warn("received unauthenticated interledger packet, discarding")
          return EMPTY_UINT8ARRAY
        }

        processIncomingPacketActor.tell("handle", {
          sourceIlpAddress: `${ilpAllocationScheme}.das.${subnetId}.${sender}`,
          ledgerAccountPath: `peer/${subnetId}.${sender}/interledger`,
          serializedPacket: content.signed.packet,
          requestId: content.signed.requestId,
        })

        return EMPTY_UINT8ARRAY
      },
    }
  })
