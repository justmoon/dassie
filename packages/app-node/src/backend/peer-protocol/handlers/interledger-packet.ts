import { createLogger } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

import { EMPTY_UINT8ARRAY } from "../../../common/constants/general"
import { subnetBalanceMapStore } from "../../balances/stores/subnet-balance-map"
import { configSignal } from "../../config"
import { IlpType } from "../../ilp-connector/ilp-packet-codec"
import { incomingIlpPacketTopic } from "../../ilp-connector/topics/incoming-ilp-packet"
import subnetModules from "../../subnets/modules"
import type { IncomingPeerMessageEvent } from "../actions/handle-peer-message"

const logger = createLogger("das:node:handle-interledger-packet")

export const handleInterledgerPacket = () =>
  createActor((sig) => {
    const incomingIlpPacketTopicValue = sig.use(incomingIlpPacketTopic)
    const { ilpAllocationScheme } = sig.getKeys(configSignal, [
      "ilpAllocationScheme",
    ])
    const balanceMap = sig.use(subnetBalanceMapStore)

    return ({
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

      if (packet.type === IlpType.Fulfill) {
        balanceMap.adjustBalance(subnetId, -packet.prepare.amount)
      }

      incomingIlpPacketTopicValue.emit(incomingPacketEvent)

      return EMPTY_UINT8ARRAY
    }
  })
