import { createLogger } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

import { EMPTY_UINT8ARRAY } from "../../../common/constants/general"
import { subnetBalanceMapStore } from "../../balances/stores/subnet-balance-map"
import { configSignal } from "../../config"
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

    const handleInterledgerPacketAsync = async ({
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
        return
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

      await subnetModule.processIncomingPacket({
        subnetId,
        balanceMap,
        packet: incomingPacketEvent.packet,
      })

      incomingIlpPacketTopicValue.emit(incomingPacketEvent)
    }

    return (parameters: IncomingPeerMessageEvent<"interledgerPacket">) => {
      handleInterledgerPacketAsync(parameters).catch((error: unknown) => {
        logger.error("error while handling interledger packet", {
          error,
        })
      })

      return EMPTY_UINT8ARRAY
    }
  })
