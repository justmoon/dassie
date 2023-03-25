import {
  Actor,
  ActorFactory,
  createActor,
  createTopic,
} from "@dassie/lib-reactive"

import { handleInterledgerPacket } from "../handlers/interledger-packet"
import { handleLinkStateRequest } from "../handlers/link-state-request"
import { handleLinkStateUpdate } from "../handlers/link-state-update"
import { handlePeeringRequest } from "../handlers/peering-request"
import type { PeerMessage } from "../peer-schema"

export interface IncomingPeerMessageEvent<
  TType extends PeerMessageType = PeerMessageType
> {
  message: PeerMessage & {
    content: { value: { type: TType; value: PeerMessageContent<TType> } }
  }
  authenticated: boolean
  asUint8Array: Uint8Array
}

export type PeerMessageType = PeerMessage["content"]["value"]["type"]

export type PeerMessageContent<T extends PeerMessageType> = Extract<
  PeerMessage["content"]["value"],
  { type: T }
>["value"]

export const incomingPeerMessageTopic = () =>
  createTopic<IncomingPeerMessageEvent>()

type AllPeerMessageHandlerFactories = {
  [K in PeerMessageType]: ActorFactory<
    (parameters: IncomingPeerMessageEvent<K>) => Uint8Array
  >
}

type AllPeerMessageHandlers = {
  [K in PeerMessageType]: Actor<
    (parameters: IncomingPeerMessageEvent<K>) => Uint8Array
  >
}

const HANDLERS: AllPeerMessageHandlerFactories = {
  peeringRequest: handlePeeringRequest,
  linkStateUpdate: handleLinkStateUpdate,
  interledgerPacket: handleInterledgerPacket,
  linkStateRequest: handleLinkStateRequest,
}

export const handlePeerMessage = () =>
  createActor((sig) => {
    for (const handler of Object.values(
      HANDLERS as Record<string, ActorFactory<unknown>>
    )) {
      sig.run(handler, undefined, { register: true })
    }

    const handlers = Object.fromEntries(
      Object.entries(HANDLERS as Record<string, ActorFactory<unknown>>).map(
        ([key, value]) => [key, sig.use(value)]
      )
    ) as unknown as AllPeerMessageHandlers

    return async <TType extends PeerMessageType>(
      parameters: IncomingPeerMessageEvent<TType>
    ) => {
      const type: TType = parameters.message.content.value.type

      return await handlers[type].ask(parameters)
    }
  })
