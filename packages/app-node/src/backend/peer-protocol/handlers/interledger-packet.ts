import { createLogger } from "@dassie/lib-logger"

import { subnetBalanceMapStore } from "../../balances/stores/subnet-balance-map"
import { configSignal } from "../../config"
import { incomingIlpPacketTopic } from "../../ilp-connector/topics/incoming-ilp-packet"
import subnetModules from "../../subnets/modules"
import type {
  IncomingPeerMessageHandlerParameters,
  PeerMessageContent,
} from "../handle-peer-message"

const logger = createLogger("das:node:handle-interledger-packet")

export const handleInterledgerPacket = (
  content: PeerMessageContent<"interledgerPacket">,
  parameters: IncomingPeerMessageHandlerParameters
) => {
  handleInterledgerPacketAsync(content, parameters).catch((error: unknown) => {
    logger.error("error while handling interledger packet", {
      error,
    })
  })
}

const handleInterledgerPacketAsync = async (
  content: PeerMessageContent<"interledgerPacket">,
  {
    message: { sender, subnetId },
    reactor,
    authenticated,
  }: IncomingPeerMessageHandlerParameters
) => {
  if (!authenticated) {
    logger.warn("received unauthenticated interledger packet, discarding")
    return
  }

  logger.debug("handle interledger packet", {
    subnet: subnetId,
    from: sender,
  })

  const incomingIlpPacketTopicValue = reactor.use(incomingIlpPacketTopic)

  const { ilpAllocationScheme } = reactor.use(configSignal).read()
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
    balanceMap: reactor.use(subnetBalanceMapStore),
    packet: incomingPacketEvent.packet,
  })

  incomingIlpPacketTopicValue.emit(incomingPacketEvent)
}
