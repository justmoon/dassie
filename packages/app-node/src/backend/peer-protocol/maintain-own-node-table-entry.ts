import { createLogger } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

import { configSignal } from "../config"
import { signerService } from "../crypto/signer"
import { compareSetToArray, compareSets } from "../utils/compare-sets"
import { peersComputation } from "./computed/peers"
import { peerNodeInfo, signedPeerNodeInfo } from "./peer-schema"
import type { PerSubnetParameters } from "./run-per-subnet-effects"
import { nodeTableStore, parseNodeKey } from "./stores/node-table"

const logger = createLogger("das:node:maintain-own-node-table-entry")

export const maintainOwnNodeTableEntry = () =>
  createActor(async (sig, parameters: PerSubnetParameters) => {
    const { subnetId } = parameters
    const signer = sig.get(signerService)

    if (!signer) return

    // Get the current peers and re-run the effect iff the IDs of the peers change.
    const peers = sig.get(peersComputation, (peersSet) => peersSet, compareSets)

    const { nodeId, url } = sig.getKeys(configSignal, ["nodeId", "url"])
    const ownNodeTableEntry = sig
      .use(nodeTableStore)
      .read()
      .get(`${subnetId}.${nodeId}`)

    if (
      ownNodeTableEntry == null ||
      !compareSetToArray(peers, ownNodeTableEntry.neighbors)
    ) {
      const sequence = BigInt(Date.now())
      const peerIds = [...peers].map((peer) => parseNodeKey(peer)[1])
      const publicKey = await signer.getPublicKey()

      const peerNodeInfoResult = peerNodeInfo.serialize({
        subnetId,
        nodeId,
        nodePublicKey: publicKey,
        url,
        sequence,
        entries: peerIds.map((peerId) => ({
          type: "neighbor",
          value: { nodeId: peerId },
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
          url,
          sequence,
          scheduledRetransmitTime: Date.now(),
          updateReceivedCounter: 0,
          lastLinkStateUpdate: message.value,
          peerState: { id: "none" },
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
  })
