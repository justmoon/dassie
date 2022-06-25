import type { XenMessage } from "../codecs/xen-message"
import { createTopic } from "../services/message-broker"

export const incomingXenMessageBufferTopic = createTopic<Buffer>(
  "incomingXenMessageBufferTopic"
)
export const incomingXenMessageTopic = createTopic<XenMessage>(
  "incomingXenMessageTopic"
)
