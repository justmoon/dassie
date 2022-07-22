import { createTopic } from "@xen-ilp/lib-reactive"

export interface GlobalFirehoseMessage {
  nodeId: string
  topic: string
  messageId: number
}

export const globalFirehoseTopic = () => createTopic<GlobalFirehoseMessage>()
