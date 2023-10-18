import { Promisable } from "type-fest"

import { type Actor, createActor, createTopic } from "@dassie/lib-reactive"

import { HandleInterledgerPacketActor } from "../handlers/interledger-packet"
import { HandleLinkStateRequestActor } from "../handlers/link-state-request"
import { HandleLinkStateUpdateActor } from "../handlers/link-state-update"
import { HandleNodeListRequestActor } from "../handlers/node-list-request"
import { HandlePeeringRequestActor } from "../handlers/peering-request"
import { HandleRegistrationActor } from "../handlers/registration"
import { HandleSettlementActor } from "../handlers/settlement"
import { HandleSettlementSchemeModuleMessageActor } from "../handlers/settlement-scheme-module-message"
import type { PeerMessage } from "../peer-schema"
import { PeerState } from "../stores/node-table"

export interface IncomingPeerMessageEvent<
  TType extends PeerMessageType = PeerMessageType,
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

export const IncomingPeerMessageTopic = () =>
  createTopic<IncomingPeerMessageEvent>()

type AllPeerMessageHandlers = {
  [K in PeerMessageType]: Actor<{
    handle: (parameters: IncomingPeerMessageEvent<K>) => Promisable<Uint8Array>
  }>
}

export const HandlePeerMessageActor = () =>
  createActor((sig) => {
    const handlers: AllPeerMessageHandlers = {
      peeringRequest: sig.use(HandlePeeringRequestActor),
      linkStateUpdate: sig.use(HandleLinkStateUpdateActor),
      interledgerPacket: sig.use(HandleInterledgerPacketActor),
      linkStateRequest: sig.use(HandleLinkStateRequestActor),
      settlement: sig.use(HandleSettlementActor),
      settlementSchemeModuleMessage: sig.use(
        HandleSettlementSchemeModuleMessageActor,
      ),
      registration: sig.use(HandleRegistrationActor),
      nodeListRequest: sig.use(HandleNodeListRequestActor),
    }

    for (const handler of Object.values(handlers)) {
      handler.run(sig.reactor, sig)
    }

    return {
      handle: async <TType extends PeerMessageType>(
        parameters: IncomingPeerMessageEvent<TType>,
      ) => {
        const type: TType = parameters.message.content.value.type

        return await handlers[type].ask("handle", parameters)
      },
    }
  })
