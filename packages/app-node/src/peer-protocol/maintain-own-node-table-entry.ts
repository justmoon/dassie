import { map } from "iterative"

import { createLogger } from "@dassie/lib-logger"
import type { EffectContext } from "@dassie/lib-reactive"

import { configStore } from "../config"
import { signerService } from "../crypto/signer"
import { compareKeysToArray, compareSetOfKeys } from "../utils/compare-sets"
import { peerNodeInfo, signedPeerNodeInfo } from "./peer-schema"
import { addNode, nodeTableStore, updateNode } from "./stores/node-table"
import { peerTableStore } from "./stores/peer-table"

const logger = createLogger("das:node:maintain-own-node-table-entry")

export const maintainOwnNodeTableEntry = async (sig: EffectContext) => {
  const { nodeId, port } = sig.getKeys(configStore, ["nodeId", "port"])
  const signer = sig.get(signerService)

  if (!signer) return

  // Get the current peers and re-run the effect iff the IDs of the peers change.
  const peers = sig.get(
    peerTableStore,
    (peerTable) => peerTable,
    compareSetOfKeys
  )

  const ownNodeTableEntry = sig.get(nodeTableStore, (nodeTable) =>
    nodeTable.get(nodeId)
  )

  if (
    ownNodeTableEntry == null ||
    !compareKeysToArray(peers, ownNodeTableEntry.neighbors)
  ) {
    const sequence = BigInt(Date.now())
    const peerIds = [...peers.values()].map((peer) => peer.nodeId)

    const peerNodeInfoResult = peerNodeInfo.serialize({
      nodeId: nodeId,
      publicKey: await signer.getPublicKey(),
      url: `https://${nodeId}.localhost:${port}`,
      sequence,
      entries: [
        ...map(peers.values(), (peer) => ({
          neighbor: { nodeId: peer.nodeId },
        })),
      ],
    })

    if (!peerNodeInfoResult.success) {
      logger.warn("Failed to serialize link state update signed portion", {
        error: peerNodeInfoResult.failure,
      })
      return
    }

    const signature = await signer.signWithDassieKey(peerNodeInfoResult.value)
    const message = signedPeerNodeInfo.serialize({
      signed: peerNodeInfoResult.value,
      signature,
    })

    if (!message.success) {
      logger.warn("Failed to serialize link state update message", {
        error: message.failure,
      })
      return
    }

    sig.use(nodeTableStore).update(
      ownNodeTableEntry == undefined
        ? addNode({
            nodeId: nodeId,
            neighbors: peerIds,
            sequence,
            scheduledRetransmitTime: Date.now(),
            updateReceivedCounter: 0,
            lastLinkStateUpdate: message.value,
          })
        : updateNode(nodeId, {
            neighbors: peerIds,
            sequence,
            scheduledRetransmitTime: Date.now(),
            updateReceivedCounter: 0,
            lastLinkStateUpdate: message.value,
          })
    )
  }
}
