import { createActor } from "@dassie/lib-reactive"

import { EMPTY_UINT8ARRAY } from "../../../common/constants/general"
import { IlpAllocationSchemeSignal } from "../../config/computed/ilp-allocation-scheme"
import { ProcessPacketActor } from "../../ilp-connector/process-packet"
import { PeerEndpointInfo } from "../../ilp-connector/senders/send-peer-packets"
import { peerProtocol as logger } from "../../logger/instances"
import type { IncomingPeerMessageEvent } from "../actors/handle-peer-message"

export const HandleInterledgerPacketActor = () =>
  createActor((sig) => {
    const ilpAllocationScheme = sig.get(IlpAllocationSchemeSignal)
    const processIncomingPacketActor = sig.use(ProcessPacketActor)

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
            "received interledger packet from unpeered node, discarding",
          )
          return EMPTY_UINT8ARRAY
        }

        const { settlementSchemeId } = peerState

        const endpointInfo: PeerEndpointInfo = {
          type: "peer",
          nodeId: sender,
          ilpAddress: `${ilpAllocationScheme}.das.${sender}`,
          accountPath: `${settlementSchemeId}/peer/${sender}/interledger`,
        }

        processIncomingPacketActor.tell("handle", {
          sourceEndpointInfo: endpointInfo,
          serializedPacket: content.signed.packet,
          requestId: content.signed.requestId,
        })

        return EMPTY_UINT8ARRAY
      },
    }
  })
