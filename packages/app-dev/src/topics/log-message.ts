import { createTopic } from "@xen-ilp/lib-events"
import type { SerializableLogLine } from "@xen-ilp/lib-logger"

export interface NodeLogLine extends SerializableLogLine {
  node: string
}

export const logLineTopic = createTopic<NodeLogLine>("logLine")
