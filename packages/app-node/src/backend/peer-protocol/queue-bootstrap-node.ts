import { hexToBytes } from "@noble/hashes/utils"

import { createActor } from "@dassie/lib-reactive"

import { environmentConfigSignal } from "../config/environment-config"
import { nodeIdSignal } from "../ilp-connector/computed/node-id"
import type { PerSettlementSchemeParameters } from "../settlement-schemes/manage-settlement-scheme-instances"
import { nodeDiscoveryQueueStore } from "./stores/node-discovery-queue"
import { nodeTableStore } from "./stores/node-table"

export const queueBootstrapNodes = () =>
  createActor((sig, { settlementSchemeId }: PerSettlementSchemeParameters) => {
    const settlementSchemeConfig = sig.get(environmentConfigSignal, (state) =>
      state.initialSettlementSchemes.find(({ id }) => id === settlementSchemeId)
    )
    const ownNodeId = sig.get(nodeIdSignal)

    if (!settlementSchemeConfig) {
      throw new Error(`Subnet '${settlementSchemeId}' is not configured`)
    }

    const candidates = settlementSchemeConfig.bootstrapNodes.filter(
      ({ nodeId }) => nodeId !== ownNodeId
    )

    const nodes = sig.use(nodeTableStore).read()

    for (const candidate of candidates) {
      const node = nodes.get(candidate.nodeId)

      if (!node) {
        sig.use(nodeTableStore).addNode({
          nodeId: candidate.nodeId,
          url: candidate.url,
          alias: candidate.alias,
          nodePublicKey: hexToBytes(candidate.nodePublicKey),
          linkState: {
            sequence: 0n,
            lastUpdate: undefined,
            updateReceivedCounter: 0,
            scheduledRetransmitTime: 0,
            neighbors: [],
            settlementSchemes: [],
          },
          peerState: { id: "none" },
        })
      }

      if (!node?.linkState) {
        sig
          .use(nodeDiscoveryQueueStore)
          .addNode(candidate.nodeId, candidate.nodeId)
      }
    }
  })
