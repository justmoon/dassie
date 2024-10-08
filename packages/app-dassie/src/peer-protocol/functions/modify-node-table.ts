import type { Infer } from "@dassie/lib-oer"
import type { Reactor } from "@dassie/lib-reactive"
import { UnreachableCaseError } from "@dassie/lib-type-utils"

import { LINK_STATE_MAX_UPDATE_RETRANSMIT_DELAY } from "../constants/timings"
import { peerNodeInfo } from "../peer-schema"
import { type NodeTableEntry, NodeTableStore } from "../stores/node-table"
import { parseLinkStateEntries } from "../utils/parse-link-state-entries"

export type AddNodeParameters = Pick<NodeTableEntry, "nodeId">

export type RetransmitType = "immediately" | "scheduled" | "never"

export interface ProcessLinkStateParameters {
  linkStateBytes: Uint8Array
  linkState: Omit<Infer<typeof peerNodeInfo>, "type">
  retransmit: RetransmitType
}

const getRetransmitDelay = (retransmit: RetransmitType) => {
  switch (retransmit) {
    case "immediately": {
      return 0
    }
    case "never": {
      return Number.POSITIVE_INFINITY
    }
    case "scheduled": {
      return Math.ceil(Math.random() * LINK_STATE_MAX_UPDATE_RETRANSMIT_DELAY)
    }
    default: {
      throw new UnreachableCaseError(retransmit)
    }
  }
}

export const ProcessLinkState = (reactor: Reactor) => {
  const nodeTableStore = reactor.use(NodeTableStore)

  return function processLinkState({
    linkStateBytes,
    linkState,
    retransmit = "scheduled",
  }: ProcessLinkStateParameters) {
    const { nodeId, url, alias, publicKey, sequence, entries } = linkState

    const node = nodeTableStore.read().get(nodeId)
    if (!node) {
      return
    }

    if (sequence === node.linkState?.sequence && retransmit === "scheduled") {
      nodeTableStore.act.updateNode(nodeId, {
        linkState: {
          ...node.linkState,
          updateReceivedCounter: node.linkState.updateReceivedCounter + 1,
        },
      })
    }

    if (sequence <= (node.linkState?.sequence ?? 0n)) {
      return
    }

    const { neighbors, settlementSchemes } = parseLinkStateEntries(entries)

    nodeTableStore.act.updateNode(nodeId, {
      linkState: {
        lastUpdate: linkStateBytes,
        sequence,
        publicKey,
        url,
        alias,
        neighbors,
        settlementSchemes,
        updateReceivedCounter: 1,
        scheduledRetransmitTime: Date.now() + getRetransmitDelay(retransmit),
      },
    })
  }
}
