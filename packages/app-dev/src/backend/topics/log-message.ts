import type { SerializableLogLine } from "@xen-ilp/lib-logger"
import { createTopic } from "@xen-ilp/lib-reactive"

export interface NodeLogLine extends SerializableLogLine {
  node: string
}

export const logLineTopic = createTopic<NodeLogLine>("logLine")
