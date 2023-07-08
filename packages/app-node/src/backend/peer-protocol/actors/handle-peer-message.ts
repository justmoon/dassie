import { Promisable } from "type-fest"

import { type Actor, createActor, createTopic } from "@dassie/lib-reactive"

import { handleInterledgerPacket } from "../handlers/interledger-packet"
import { handleLinkStateRequest } from "../handlers/link-state-request"
import { handleLinkStateUpdate } from "../handlers/link-state-update"
import { handlePeeringRequest } from "../handlers/peering-request"
import { handleSettlement } from "../handlers/settlement"
import { handleSettlementSchemeModuleMessage } from "../handlers/settlement-scheme-module-message"
import type { PeerMessage } from "../peer-schema"
import { PeerState } from "../stores/node-table"

export interface IncomingPeerMessageEvent<
  TType extends PeerMessageType = PeerMessageType
> {
  message: PeerMessage & {
    content: { value: { type: TType; value: PeerMessageContent<TType> } }
  }
  authenticated: boolean
  peerState: PeerState | undefined
  asUint8Array: Uint8Array
}

export type PeerMessageType = PeerMessage["content"]["value"]["type"]

export type PeerMessageContent<T extends PeerMessageType> = Extract<
  PeerMessage["content"]["value"],
  { type: T }
>["value"]

export const incomingPeerMessageTopic = () =>
  createTopic<IncomingPeerMessageEvent>()

type AllPeerMessageHandlers = {
  [K in PeerMessageType]: Actor<{
    handle: (parameters: IncomingPeerMessageEvent<K>) => Promisable<Uint8Array>
  }>
}

export const handlePeerMessage = () =>
  createActor((sig) => {
    const handlers: AllPeerMessageHandlers = {
      peeringRequest: sig.use(handlePeeringRequest),
      linkStateUpdate: sig.use(handleLinkStateUpdate),
      interledgerPacket: sig.use(handleInterledgerPacket),
      linkStateRequest: sig.use(handleLinkStateRequest),
      settlement: sig.use(handleSettlement),
      settlementSchemeModuleMessage: sig.use(
        handleSettlementSchemeModuleMessage
      ),
    }

    for (const handler of Object.values(handlers)) {
      handler.run(sig)
    }

    return {
      handle: async <TType extends PeerMessageType>(
        parameters: IncomingPeerMessageEvent<TType>
      ) => {
        const type: TType = parameters.message.content.value.type

        return await handlers[type].ask("handle", parameters)
      },
    }
  })
