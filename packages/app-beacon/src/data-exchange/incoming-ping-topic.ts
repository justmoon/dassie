import { createTopic } from "@dassie/lib-reactive"

export interface IncomingPingSubnet {
  subnetId: string
}

export interface IncomingPing {
  nodeId: string
  url: string
  subnets: IncomingPingSubnet[]
}

export const incomingPingTopic = () => createTopic<IncomingPing>()
