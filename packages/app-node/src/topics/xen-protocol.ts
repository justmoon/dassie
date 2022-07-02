import { createTopic } from "@xen-ilp/lib-events"

import type { XenMessage } from "../codecs/xen-message"

export const incomingXenMessageBufferTopic = createTopic<Buffer>(
  "incomingXenMessageBufferTopic"
)
export const incomingXenMessageTopic = createTopic<XenMessage>(
  "incomingXenMessageTopic"
)
