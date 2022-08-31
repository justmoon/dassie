import type { EffectContext } from "@dassie/lib-reactive"

import { incomingPingTopic } from "./incoming-ping-topic"

export interface TrackedNode {
  nodeId: string
  url: string
}

export const trackedNodes = new Map<string, Map<string, TrackedNode>>()

export const trackNodes = (sig: EffectContext) => {
  sig.on(incomingPingTopic, (ping) => {
    for (const subnet of ping.subnets) {
      let subnetSet = trackedNodes.get(subnet.subnetId)

      if (!subnetSet) {
        subnetSet = new Map()
        trackedNodes.set(subnet.subnetId, subnetSet)
      }

      subnetSet.set(ping.nodeId, {
        nodeId: ping.nodeId,
        url: ping.url,
      })
    }
  })
}
