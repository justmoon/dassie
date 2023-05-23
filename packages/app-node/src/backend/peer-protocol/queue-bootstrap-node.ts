import { hexToBytes } from "@noble/hashes/utils"

import { createActor } from "@dassie/lib-reactive"

import { configSignal } from "../config"
import { nodeIdSignal } from "../ilp-connector/computed/node-id"
import type { PerSubnetParameters } from "../subnets/manage-subnet-instances"
import { nodeDiscoveryQueueStore } from "./stores/node-discovery-queue"
import { nodeTableStore } from "./stores/node-table"

export const queueBootstrapNodes = () =>
  createActor((sig, { subnetId }: PerSubnetParameters) => {
    const subnetConfig = sig.get(configSignal, (state) =>
      state.initialSubnets.find(({ id }) => id === subnetId)
    )
    const ownNodeId = sig.get(nodeIdSignal)

    if (!subnetConfig) {
      throw new Error(`Subnet '${subnetId}' is not configured`)
    }

    const candidates = subnetConfig.bootstrapNodes.filter(
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
            subnets: [],
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
