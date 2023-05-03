import { createLogger } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

import { EMPTY_UINT8ARRAY } from "../../../common/constants/general"
import { peerBalanceMapStore } from "../../balances/stores/peer-balance-map"
import { configSignal } from "../../config"
import { IlpType } from "../../ilp-connector/ilp-packet-codec"
import { incomingIlpPacketTopic } from "../../ilp-connector/topics/incoming-ilp-packet"
import subnetModules from "../../subnets/modules"
import type { IncomingPeerMessageEvent } from "../actions/handle-peer-message"
import type { NodeTableKey } from "../stores/node-table"

const logger = createLogger("das:node:handle-interledger-packet")

export const handleInterledgerPacket = () =>
  createActor((sig) => {
    const incomingIlpPacketTopicValue = sig.use(incomingIlpPacketTopic)
    const { ilpAllocationScheme } = sig.getKeys(configSignal, [
      "ilpAllocationScheme",
    ])
    const balanceMap = sig.use(peerBalanceMapStore)

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

        logger.debug("handle interledger packet", {
          subnet: subnetId,
          from: sender,
        })

        const incomingPacketEvent = incomingIlpPacketTopicValue.prepareEvent({
          source: `${ilpAllocationScheme}.das.${subnetId}.${sender}`,
          packet: content.signed.packet,
          requestId: content.signed.requestId,
        })

        const subnetModule = subnetModules[subnetId]

        if (!subnetModule) {
          throw new Error(`unknown subnet: ${subnetId}`)
        }

        const { packet } = incomingPacketEvent

        const peerKey: NodeTableKey = `${subnetId}.${sender}`
        switch (packet.type) {
          case IlpType.Prepare: {
            if (packet.amount > 0n) {
              balanceMap.handleIncomingPacketPrepared(peerKey, packet.amount)
            }
            break
          }
          case IlpType.Fulfill: {
            if (packet.prepare.amount > 0n) {
              balanceMap.handleOutgoingPacketFulfilled(
                peerKey,
                packet.prepare.amount
              )
            }
            break
          }
          case IlpType.Reject: {
            if (packet.prepare.amount > 0n) {
              balanceMap.handleOutgoingPacketRejected(
                peerKey,
                packet.prepare.amount
              )
            }
            break
          }
        }

        incomingIlpPacketTopicValue.emit(incomingPacketEvent)

        return EMPTY_UINT8ARRAY
      },
    }
  })
