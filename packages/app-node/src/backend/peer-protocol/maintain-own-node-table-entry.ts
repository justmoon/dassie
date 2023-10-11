import { createActor } from "@dassie/lib-reactive"

import { DatabaseConfigStore } from "../config/database-config"
import { NodePublicKeySignal } from "../crypto/computed/node-public-key"
import { SignerActor } from "../crypto/signer"
import { NodeIdSignal } from "../ilp-connector/computed/node-id"
import { peerProtocol as logger } from "../logger/instances"
import { ActiveSettlementSchemesSignal } from "../settlement-schemes/signals/active-settlement-schemes"
import { compareSetToArray } from "../utils/compare-sets"
import { PeersSignal } from "./computed/peers"
import { peerNodeInfo, signedPeerNodeInfo } from "./peer-schema"
import { NodeTableStore } from "./stores/node-table"

export const MaintainOwnNodeTableEntryActor = () =>
  createActor(async (sig) => {
    const signer = sig.get(SignerActor)

    if (!signer) return

    // Get the current peers and re-run the actor if they change
    const peers = sig.get(PeersSignal)

    const settlementSchemes = sig.get(ActiveSettlementSchemesSignal)

    const nodeId = sig.get(NodeIdSignal)
    const nodePublicKey = sig.get(NodePublicKeySignal)
    const { url, alias } = sig.get(DatabaseConfigStore)
    const ownNodeTableEntry = sig.use(NodeTableStore).read().get(nodeId)

    if (
      ownNodeTableEntry == null ||
      !compareSetToArray(peers, ownNodeTableEntry.linkState.neighbors)
    ) {
      const sequence = BigInt(Date.now())
      const peerIds = [...peers]
      const settlementSchemeIds = [...settlementSchemes]

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
          ...settlementSchemeIds.map((settlementSchemeId) => ({
            type: "settlementScheme" as const,
            value: { settlementSchemeId },
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
        sig.use(NodeTableStore).addNode({
          nodeId,
          nodePublicKey,
          url,
          alias,
          linkState: {
            neighbors: peerIds,
            settlementSchemes: settlementSchemeIds,
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
        sig.use(NodeTableStore).updateNode(nodeId, {
          linkState: {
            neighbors: peerIds,
            settlementSchemes: settlementSchemeIds,
            sequence,
            scheduledRetransmitTime: Date.now(),
            updateReceivedCounter: 0,
            lastUpdate: message.value,
          },
        })
      }
    }
  })
