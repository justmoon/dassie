import { createLogger } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

import { configSignal } from "../config"
import { nodePublicKeySignal } from "../crypto/computed/node-public-key"
import { signerService } from "../crypto/signer"
import { nodeIdSignal } from "../ilp-connector/computed/node-id"
import { compareSetToArray } from "../utils/compare-sets"
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

    // Get the current peers and re-run the actor if they change
    const peers = sig.get(peersComputation)

    const nodeId = sig.get(nodeIdSignal)
    const nodePublicKey = sig.get(nodePublicKeySignal)
    const { url } = sig.getKeys(configSignal, ["url"])
    const ownNodeTableEntry = sig
      .use(nodeTableStore)
      .read()
      .get(`${subnetId}.${nodeId}`)

    if (
      ownNodeTableEntry == null ||
      !compareSetToArray(peers, ownNodeTableEntry.linkState.neighbors)
    ) {
      const sequence = BigInt(Date.now())
      const peerIds = [...peers].map((peer) => parseNodeKey(peer)[1])

      const peerNodeInfoResult = peerNodeInfo.serialize({
        subnetId,
        nodeId,
        nodePublicKey,
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
          nodePublicKey,
          url,
          linkState: {
            neighbors: peerIds,
            sequence,
            scheduledRetransmitTime: Date.now(),
            updateReceivedCounter: 0,
            lastUpdate: message.value,
          },
          peerState: { id: "none" },
        })
      } else {
        logger.debug("updating own node table entry", {
          sequence,
          neighbors: peerIds.join(","),
        })
        sig.use(nodeTableStore).updateNode(`${subnetId}.${nodeId}`, {
          linkState: {
            neighbors: peerIds,
            sequence,
            scheduledRetransmitTime: Date.now(),
            updateReceivedCounter: 0,
            lastUpdate: message.value,
          },
        })
      }
    }
  })
