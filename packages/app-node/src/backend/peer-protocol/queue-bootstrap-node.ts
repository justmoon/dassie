import { createActor } from "@dassie/lib-reactive"

import { EnvironmentConfigSignal } from "../config/environment-config"
import { NodeIdSignal } from "../ilp-connector/computed/node-id"
import { NodeTableStore } from "./stores/node-table"

export const QueueBootstrapNodesActor = () =>
  createActor((sig) => {
    const bootstrapNodes = sig.get(
      EnvironmentConfigSignal,
      (state) => state.bootstrapNodes,
    )
    const ownNodeId = sig.get(NodeIdSignal)

    const candidates = bootstrapNodes.filter(
      ({ nodeId }) => nodeId !== ownNodeId,
    )

    const nodes = sig.use(NodeTableStore).read()

    for (const candidate of candidates) {
      const node = nodes.get(candidate.nodeId)

      const publicKeyBuffer = Buffer.from(candidate.nodePublicKey, "base64url")
      if (!node) {
        sig.use(NodeTableStore).addNode({
          nodeId: candidate.nodeId,
          url: candidate.url,
          alias: candidate.alias,
          nodePublicKey: new Uint8Array(
            publicKeyBuffer.buffer,
            publicKeyBuffer.byteOffset,
            publicKeyBuffer.byteLength,
          ),
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
    }
  })
