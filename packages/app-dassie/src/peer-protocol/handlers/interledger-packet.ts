import { parseIlpPacket } from "@dassie/lib-protocol-ilp"

import type { DassieReactor } from "../../base/types/dassie-base"
import { ProcessPacket } from "../../ilp-connector/functions/process-packet"
import type { PeerEndpointInfo } from "../../ilp-connector/senders/send-peer-packets"
import { GetLedgerIdForSettlementScheme } from "../../ledgers/functions/get-ledger-id"
import { peerProtocol as logger } from "../../logger/instances"
import type { PeerMessageHandler } from "../functions/handle-peer-message"

export const HandleInterledgerPacket = ((reactor: DassieReactor) => {
  const processPacket = reactor.use(ProcessPacket)
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
      accountPath: `${ledgerId}:assets/interledger/${sender}`,
    }

    processPacket({
      sourceEndpointInfo: endpointInfo,
      serializedPacket: content.signed.packet,
      parsedPacket: parseIlpPacket(content.signed.packet),
      requestId: content.signed.requestId,
    })
  }
}) satisfies PeerMessageHandler<"interledgerPacket">
