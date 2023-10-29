import { Promisable } from "type-fest"

import { InferSerialize } from "@dassie/lib-oer"
import { Factory, createActor, createTopic } from "@dassie/lib-reactive"

import { HandleInterledgerPacket } from "../handlers/interledger-packet"
import { HandleLinkStateRequest } from "../handlers/link-state-request"
import { HandleLinkStateUpdate } from "../handlers/link-state-update"
import { HandleNodeListHashRequest } from "../handlers/node-list-hash-request"
import { HandleNodeListRequest } from "../handlers/node-list-request"
import { HandlePeeringRequest } from "../handlers/peering-request"
import { HandleRegistration } from "../handlers/registration"
import { HandleSettlement } from "../handlers/settlement"
import { HandleSettlementMessage } from "../handlers/settlement-message"
import type { PeerMessage } from "../peer-schema"
import { peerMessageResponse } from "../peer-schema"
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

export type PeerMessageHandler<TType extends PeerMessageType> = Factory<
  (
    parameters: IncomingPeerMessageEvent<TType>,
  ) => Promisable<InferSerialize<(typeof peerMessageResponse)[TType]>>
>

export type PeerMessageType = PeerMessage["content"]["value"]["type"]

export type PeerMessageContent<T extends PeerMessageType> = Extract<
  PeerMessage["content"]["value"],
  { type: T }
>["value"]

export const IncomingPeerMessageTopic = () =>
  createTopic<IncomingPeerMessageEvent>()

type PeerMessageHandlers = {
  [K in PeerMessageType]: ReturnType<PeerMessageHandler<K>>
}

export const HandlePeerMessageActor = () =>
  createActor((sig) => {
    const handlers: PeerMessageHandlers = {
      peeringRequest: sig.use(HandlePeeringRequest),
      linkStateUpdate: sig.use(HandleLinkStateUpdate),
      interledgerPacket: sig.use(HandleInterledgerPacket),
      linkStateRequest: sig.use(HandleLinkStateRequest),
      settlement: sig.use(HandleSettlement),
      settlementMessage: sig.use(HandleSettlementMessage),
      registration: sig.use(HandleRegistration),
      nodeListRequest: sig.use(HandleNodeListRequest),
      nodeListHashRequest: sig.use(HandleNodeListHashRequest),
    }

    return {
      handle: async <TType extends PeerMessageType>(
        parameters: IncomingPeerMessageEvent<TType>,
      ) => {
        const type: TType = parameters.message.content.value.type

        const result = await handlers[type](parameters)

        // eslint-disable-next-line @typescript-eslint/no-unsafe-argument, @typescript-eslint/no-explicit-any
        return peerMessageResponse[type].serializeOrThrow(result as any)
      },
    }
  })
