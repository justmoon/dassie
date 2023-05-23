import { createLogger } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

import { configSignal } from "../config"
import { nodePublicKeySignal } from "../crypto/computed/node-public-key"
import { signerService } from "../crypto/signer"
import { nodeIdSignal } from "../ilp-connector/computed/node-id"
import { activeSubnetsSignal } from "../subnets/signals/active-subnets"
import { compareSetToArray } from "../utils/compare-sets"
import { peersComputation } from "./computed/peers"
import { peerNodeInfo, signedPeerNodeInfo } from "./peer-schema"
import { nodeTableStore } from "./stores/node-table"

const logger = createLogger("das:node:maintain-own-node-table-entry")

export const maintainOwnNodeTableEntry = () =>
  createActor(async (sig) => {
    const signer = sig.get(signerService)

    if (!signer) return

    // Get the current peers and re-run the actor if they change
    const peers = sig.get(peersComputation)

    const subnets = sig.get(activeSubnetsSignal)

    const nodeId = sig.get(nodeIdSignal)
    const nodePublicKey = sig.get(nodePublicKeySignal)
    const { url, alias } = sig.getKeys(configSignal, ["url", "alias"])
    const ownNodeTableEntry = sig.use(nodeTableStore).read().get(nodeId)

    if (
      ownNodeTableEntry == null ||
      !compareSetToArray(peers, ownNodeTableEntry.linkState.neighbors)
    ) {
      const sequence = BigInt(Date.now())
      const peerIds = [...peers]

      const peerNodeInfoResult = peerNodeInfo.serialize({
        nodeId,
        nodePublicKey,
        url,
        alias,
        sequence,
        entries: [
          ...peerIds.map((nodeId) => ({
            type: "neighbor" as const,
            value: { nodeId },
          })),
          ...subnets.map((subnetId) => ({
            type: "subnet" as const,
            value: { subnetId },
          })),
        ],
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
          nodeId,
          nodePublicKey,
          url,
          alias,
          linkState: {
            neighbors: peerIds,
            subnets,
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
        sig.use(nodeTableStore).updateNode(nodeId, {
          linkState: {
            neighbors: peerIds,
            subnets,
            sequence,
            scheduledRetransmitTime: Date.now(),
            updateReceivedCounter: 0,
            lastUpdate: message.value,
          },
        })
      }
    }
  })
