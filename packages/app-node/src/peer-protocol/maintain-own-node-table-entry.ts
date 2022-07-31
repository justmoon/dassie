import { createLogger } from "@xen-ilp/lib-logger"
import type { EffectContext } from "@xen-ilp/lib-reactive"

import { configStore } from "../config"
import { signerValue } from "../signer/signer"
import { compareKeysToArray, compareSetOfKeys } from "../utils/compare-sets"
import { peerMessage, peerSignedLinkStateUpdate } from "./peer-schema"
import { addNode, nodeTableStore, updateNode } from "./stores/node-table"
import { peerTableStore } from "./stores/peer-table"

const logger = createLogger("xen:node:maintain-own-node-table-entry")

export const maintainOwnNodeTableEntry = (sig: EffectContext) => {
  const ownNodeId = sig.get(configStore, ({ nodeId }) => nodeId)

  // Get the current peers and re-run the effect iff the IDs of the peers change.
  const peers = sig.get(
    peerTableStore,
    (peerTable) => peerTable,
    compareSetOfKeys
  )

  const ownNodeTableEntry = sig.get(nodeTableStore, (nodeTable) =>
    nodeTable.get(ownNodeId)
  )

  if (
    ownNodeTableEntry == null ||
    !compareKeysToArray(peers, ownNodeTableEntry.neighbors)
  ) {
    const sequence = BigInt(Date.now())
    const peerIds = [...peers.values()].map((peer) => peer.nodeId)

    const signer = sig.get(signerValue)
    const signedLinkStateUpdate = peerSignedLinkStateUpdate.serialize({
      nodeId: ownNodeId,
      sequence,
      neighbors: [...peers.values()],
    })

    if (!signedLinkStateUpdate.success) {
      logger.warn("Failed to serialize link state update signed portion", {
        error: signedLinkStateUpdate.failure,
      })
      return
    }

    const signature = signer.signWithXenKey(signedLinkStateUpdate.value)
    const message = peerMessage.serialize({
      linkStateUpdate: {
        signed: signedLinkStateUpdate.value,
        signature,
      },
    })

    if (!message.success) {
      logger.warn("Failed to serialize link state update message", {
        error: message.failure,
      })
      return
    }

    sig.emit(
      nodeTableStore,
      ownNodeTableEntry == undefined
        ? addNode({
            nodeId: ownNodeId,
            neighbors: peerIds,
            sequence,
            scheduledRetransmitTime: Date.now(),
            updateReceivedCounter: 0,
            lastLinkStateUpdate: message.value,
          })
        : updateNode(ownNodeId, {
            neighbors: peerIds,
            sequence,
            scheduledRetransmitTime: Date.now(),
            updateReceivedCounter: 0,
            lastLinkStateUpdate: message.value,
          })
    )
  }
}
