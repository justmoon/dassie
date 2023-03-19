import { createLogger } from "@dassie/lib-logger"
import type { Reactor } from "@dassie/lib-reactive"

import { EMPTY_UINT8ARRAY } from "../../../common/constants/general"
import { subnetBalanceMapStore } from "../../balances/stores/subnet-balance-map"
import { configSignal } from "../../config"
import { incomingIlpPacketTopic } from "../../ilp-connector/topics/incoming-ilp-packet"
import subnetModules from "../../subnets/modules"
import type {
  IncomingPeerMessageEvent,
  PeerMessageContent,
} from "../actions/handle-peer-message"

const logger = createLogger("das:node:handle-interledger-packet")

export const handleInterledgerPacket = (reactor: Reactor) => {
  const incomingIlpPacketTopicValue = reactor.use(incomingIlpPacketTopic)
  const { ilpAllocationScheme } = reactor.use(configSignal).read()
  const balanceMap = reactor.use(subnetBalanceMapStore)

  const handleInterledgerPacketAsync = async (
    content: PeerMessageContent<"interledgerPacket">,
    { message: { sender, subnetId }, authenticated }: IncomingPeerMessageEvent
  ) => {
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

  return (
    content: PeerMessageContent<"interledgerPacket">,
    parameters: IncomingPeerMessageEvent
  ) => {
    handleInterledgerPacketAsync(content, parameters).catch(
      (error: unknown) => {
        logger.error("error while handling interledger packet", {
          error,
        })
      }
    )

    return EMPTY_UINT8ARRAY
  }
}
