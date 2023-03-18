import { EffectContext, createTopic } from "@dassie/lib-reactive"

import { handleInterledgerPacket } from "../handlers/interledger-packet"
import { handleLinkStateRequest } from "../handlers/link-state-request"
import { handleLinkStateUpdate } from "../handlers/link-state-update"
import { handlePeeringRequest } from "../handlers/peering-request"
import type { PeerMessage } from "../peer-schema"

export interface IncomingPeerMessageEvent {
  message: PeerMessage
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

type AllPeerMessageHandlers = {
  [K in PeerMessageType]: (
    content: PeerMessageContent<K>,
    parameters: IncomingPeerMessageEvent
  ) => Uint8Array
}

export const handlePeerMessage = (sig: EffectContext) => {
  const handlers: AllPeerMessageHandlers = {
    peeringRequest: sig.use(handlePeeringRequest),
    linkStateUpdate: sig.use(handleLinkStateUpdate),
    interledgerPacket: sig.use(handleInterledgerPacket),
    linkStateRequest: sig.use(handleLinkStateRequest),
  }

  const runHandler = <T extends PeerMessage["content"]["value"]["type"]>(
    type: T,
    content: PeerMessageContent<T>,
    parameters: IncomingPeerMessageEvent
  ) => {
    const handler = handlers[type]
    return handler(content, parameters)
  }

  return (parameters: IncomingPeerMessageEvent): Uint8Array => {
    const content = parameters.message.content.value

    return runHandler(content.type, content.value, parameters)
  }
}
