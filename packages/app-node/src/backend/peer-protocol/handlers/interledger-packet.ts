import { Reactor } from "@dassie/lib-reactive"

import { IlpAllocationSchemeSignal } from "../../config/computed/ilp-allocation-scheme"
import { ProcessPacketActor } from "../../ilp-connector/process-packet"
import { PeerEndpointInfo } from "../../ilp-connector/senders/send-peer-packets"
import { peerProtocol as logger } from "../../logger/instances"
import { GetLedgerIdForSettlementScheme } from "../../settlement-schemes/functions/get-ledger-id"
import type { PeerMessageHandler } from "../actors/handle-peer-message"

export const HandleInterledgerPacket = ((reactor: Reactor) => {
  const ilpAllocationSchemeSignal = reactor.use(IlpAllocationSchemeSignal)
  const processIncomingPacketActor = reactor.use(ProcessPacketActor)
  const getLedgerIdForSettlementScheme = reactor.use(
    GetLedgerIdForSettlementScheme,
  )

  return ({
    message: {
      sender,
      content: {
        value: { value: content },
      },
    },
    authenticated,
    peerState,
  }) => {
    if (!authenticated) {
      logger.warn("received unauthenticated interledger packet, discarding")
      return
    }

    if (peerState?.id !== "peered") {
      logger.warn("received interledger packet from unpeered node, discarding")
      return
    }

    const { settlementSchemeId } = peerState

    const ledgerId = getLedgerIdForSettlementScheme(settlementSchemeId)

    const endpointInfo: PeerEndpointInfo = {
      type: "peer",
      nodeId: sender,
      ilpAddress: `${ilpAllocationSchemeSignal.read()}.das.${sender}`,
      accountPath: `${ledgerId}:peer/${sender}/interledger`,
    }

    processIncomingPacketActor.api.parseAndHandle.tell({
      sourceEndpointInfo: endpointInfo,
      serializedPacket: content.signed.packet,
      requestId: content.signed.requestId,
    })
  }
}) satisfies PeerMessageHandler<"interledgerPacket">
