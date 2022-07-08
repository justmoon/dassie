import { createTopic } from "@xen-ilp/lib-reactive"

import type {
  XenMessage,
  XenMessageWithOptionalSignature,
} from "../codecs/xen-message"

export const incomingXenMessageBufferTopic = createTopic<Buffer>(
  "incoming-xen-message-buffer-topic"
)
export const incomingXenMessageTopic = createTopic<XenMessage>(
  "incoming-xen-message-topic"
)

export interface MessageWithDestination<T> {
  message: T
  destination: string
}

export const outgoingUnsignedXenMessageTopic = createTopic<
  MessageWithDestination<XenMessageWithOptionalSignature>
>("outgoing-unsigned-xen-message-topic")

export const outgoingXenMessageTopic = createTopic<
  MessageWithDestination<XenMessage>
>("outgoing-xen-message-topic")

export const outgoingXenMessageBufferTopic = createTopic<
  MessageWithDestination<Buffer>
>("outgoing-xen-message-buffer-topic")
