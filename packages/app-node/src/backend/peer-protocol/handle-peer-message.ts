import { Reactor, createTopic } from "@dassie/lib-reactive"

import { handleInterledgerPacket } from "./handlers/interledger-packet"
import { handleLinkStateUpdate } from "./handlers/link-state-update"
import { handlePeeringRequest } from "./handlers/peering-request"
import type { PeerMessage } from "./peer-schema"

export interface IncomingPeerMessageEvent {
  message: PeerMessage
  authenticated: boolean
  asUint8Array: Uint8Array
}

export interface IncomingPeerMessageHandlerParameters
  extends IncomingPeerMessageEvent {
  reactor: Reactor
}

export type PeerMessageContent<
  T extends keyof PeerMessage["content"]["value"]
> = NonNullable<PeerMessage["content"]["value"][T]>

const EMPTY_BUFFER = Buffer.alloc(0)

export const incomingPeerMessageTopic = () =>
  createTopic<IncomingPeerMessageEvent>()

export const handlePeerMessage = (
  parameters: IncomingPeerMessageHandlerParameters
): Buffer => {
  const content = parameters.message.content.value

  if (content.peeringRequest) {
    handlePeeringRequest(content.peeringRequest, parameters)
  } else if (content.linkStateUpdate) {
    handleLinkStateUpdate(content.linkStateUpdate, parameters)
  } else if (content.interledgerPacket) {
    handleInterledgerPacket(content.interledgerPacket, parameters)
  }

  return EMPTY_BUFFER
}
