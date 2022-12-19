import { createLogger } from "@dassie/lib-logger"
import type { EffectContext } from "@dassie/lib-reactive"

import { configSignal } from "../config"
import { signerService } from "../crypto/signer"
import { compareKeysToArray, compareSetOfKeys } from "../utils/compare-sets"
import { peerNodeInfo, signedPeerNodeInfo } from "./peer-schema"
import type { PerSubnetParameters } from "./run-per-subnet-effects"
import { nodeTableStore } from "./stores/node-table"
import { peerTableStore } from "./stores/peer-table"

const logger = createLogger("das:node:maintain-own-node-table-entry")

export const maintainOwnNodeTableEntry = async (
  sig: EffectContext,
  parameters: PerSubnetParameters
) => {
  const { subnetId } = parameters
  const signer = sig.get(signerService)

  if (!signer) return

  // Get the current peers and re-run the effect iff the IDs of the peers change.
  const peers = sig.get(
    peerTableStore,
    (peerTable) => peerTable,
    compareSetOfKeys
  )

  const { nodeId, url } = sig.getKeys(configSignal, ["nodeId", "url"])
  const ownNodeTableEntry = sig
    .use(nodeTableStore)
    .read()
    .get(`${subnetId}.${nodeId}`)

  if (
    ownNodeTableEntry == null ||
    !compareKeysToArray(peers, ownNodeTableEntry.neighbors)
  ) {
    const sequence = BigInt(Date.now())
    const peerIds = [...peers.values()].map((peer) => peer.nodeId)
    const publicKey = await signer.getPublicKey()

    const peerNodeInfoResult = peerNodeInfo.serialize({
      subnetId,
      nodeId,
      nodePublicKey: publicKey,
      url,
      sequence,
      entries: peerIds.map((peerId) => ({
        neighbor: { nodeId: peerId },
      })),
    })

    if (!peerNodeInfoResult.success) {
      logger.warn("Failed to serialize link state update signed portion", {
        error: peerNodeInfoResult.error,
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
        error: message.error,
      })
      return
    }

    if (ownNodeTableEntry === undefined) {
      logger.debug("creating own node table entry", {
        sequence,
        neighbors: peerIds.join(","),
      })
      sig.use(nodeTableStore).addNode({
        subnetId,
        nodeId,
        neighbors: peerIds,
        nodePublicKey: publicKey,
        sequence,
        scheduledRetransmitTime: Date.now(),
        updateReceivedCounter: 0,
        lastLinkStateUpdate: message.value,
      })
    } else {
      logger.debug("updating own node table entry", {
        sequence,
        neighbors: peerIds.join(","),
      })
      sig.use(nodeTableStore).updateNode(`${subnetId}.${nodeId}`, {
        neighbors: peerIds,
        sequence,
        scheduledRetransmitTime: Date.now(),
        updateReceivedCounter: 0,
        lastLinkStateUpdate: message.value,
      })
    }
  }
}
