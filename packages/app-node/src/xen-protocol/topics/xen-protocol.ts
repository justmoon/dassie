import { createTopic } from "@xen-ilp/lib-reactive"

import type {
  XenMessage,
  XenMessageWithOptionalSignature,
} from "../codecs/xen-message"

export const incomingXenMessageBufferTopic = () => createTopic<Uint8Array>()
export const incomingXenMessageTopic = () => createTopic<XenMessage>()

export interface MessageWithDestination<T> {
  message: T
  destination: string
}

export const outgoingUnsignedXenMessageTopic = () =>
  createTopic<MessageWithDestination<XenMessageWithOptionalSignature>>()

export const outgoingXenMessageTopic = () =>
  createTopic<MessageWithDestination<XenMessage>>()

export const outgoingXenMessageBufferTopic = () =>
  createTopic<MessageWithDestination<Uint8Array>>()
