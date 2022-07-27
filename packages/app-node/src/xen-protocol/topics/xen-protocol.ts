import { createTopic } from "@xen-ilp/lib-reactive"

import type {
  XenEnvelope,
  XenEnvelopeWithOptionalSignature,
  XenMessage,
} from "../codecs/xen-message"

export const incomingXenMessageBufferTopic = () => createTopic<Uint8Array>()
export const incomingXenMessageTopic = () => createTopic<XenEnvelope>()

export interface MessageWithDestination<T> {
  envelope: T
  destination: string
}

export const outgoingUnsignedXenMessageTopic = () =>
  createTopic<MessageWithDestination<XenEnvelopeWithOptionalSignature>>()

export const outgoingXenMessageTopic = () =>
  createTopic<MessageWithDestination<XenMessage>>()

export const outgoingXenMessageBufferTopic = () =>
  createTopic<MessageWithDestination<Uint8Array>>()
