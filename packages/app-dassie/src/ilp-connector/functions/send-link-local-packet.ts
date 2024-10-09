import { nanoid } from "nanoid"

import {
  type IlpPreparePacket,
  type IlpResponsePacket,
  IlpType,
  serializeIlpPacket,
} from "@dassie/lib-protocol-ilp"
import { createDeferred } from "@dassie/lib-reactive"

import type { AccountPath } from "../../accounting/types/account-paths"
import type { DassieReactor } from "../../base/types/dassie-base"
import { OutstandingRequestsStore } from "../../local-ilp/stores/outstanding-requests"
import type { NodeId } from "../../peer-protocol/types/node-id"
import type { LocalEndpointInfo } from "../senders/send-local-packets"
import type { PeerEndpointInfo } from "../senders/send-peer-packets"
import { ProcessPreparePacket } from "./process-prepare-packet"

interface SendLinkLocalPacketParameters {
  packet: IlpPreparePacket
  peerId: NodeId
}

export function SendLinkLocalPacket(reactor: DassieReactor) {
  const processPreparePacket = reactor.use(ProcessPreparePacket)
  const outstandingRequestsStore = reactor.use(OutstandingRequestsStore)
  return async function sendLinkLocalPacket({
    packet,
    peerId,
  }: SendLinkLocalPacketParameters): Promise<IlpResponsePacket> {
    const requestId = nanoid()
    const deferred = createDeferred<IlpResponsePacket>()

    if (packet.amount !== 0n) {
      throw new Error(
        "This function only supports link-local packets with zero amount",
      )
    }

    outstandingRequestsStore.act.addRequest(requestId, deferred.resolve)

    const parsedPacket = {
      type: IlpType.Prepare,
      data: packet,
    }

    const sourceEndpointInfo: LocalEndpointInfo = {
      type: "local",
      hint: "Link Local Sender",
      localIlpAddressPart: "link-local",
    }

    const destinationEndpointInfo: PeerEndpointInfo = {
      type: "peer",
      nodeId: peerId,
      accountPath: "invalid" as AccountPath,
    }

    processPreparePacket({
      sourceEndpointInfo,
      parsedPacket,
      serializedPacket: serializeIlpPacket(parsedPacket),
      requestId,
      predeterminedOutcome: {
        destinationEndpointInfo,
        outgoingAmount: 0n,
        outgoingExpiry: packet.expiresAt,
        transfers: [],
      },
    })

    return deferred
  }
}
