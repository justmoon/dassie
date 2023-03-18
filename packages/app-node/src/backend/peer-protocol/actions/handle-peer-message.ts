import { EffectContext, createTopic } from "@dassie/lib-reactive"

import { EMPTY_UINT8ARRAY } from "../../../common/constants/general"
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

export type PeerMessageContent<
  T extends keyof PeerMessage["content"]["value"]
> = NonNullable<PeerMessage["content"]["value"][T]>

export const incomingPeerMessageTopic = () =>
  createTopic<IncomingPeerMessageEvent>()

export const handlePeerMessage =
  (sig: EffectContext) =>
  (parameters: IncomingPeerMessageEvent): Uint8Array => {
    const content = parameters.message.content.value

    if (content.peeringRequest) {
      sig.use(handlePeeringRequest)(content.peeringRequest)
    } else if (content.linkStateUpdate) {
      sig.use(handleLinkStateUpdate)(content.linkStateUpdate, parameters)
    } else if (content.interledgerPacket) {
      sig.use(handleInterledgerPacket)(content.interledgerPacket, parameters)
    } else {
      return sig.use(handleLinkStateRequest)(content.linkStateRequest)
    }

    return EMPTY_UINT8ARRAY
  }
