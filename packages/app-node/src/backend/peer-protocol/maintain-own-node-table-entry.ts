import { createActor } from "@dassie/lib-reactive"
import { bigIntMax } from "@dassie/lib-type-utils"

import { DatabaseConfigStore } from "../config/database-config"
import { NodePublicKeySignal } from "../crypto/computed/node-public-key"
import { SignerActor } from "../crypto/signer"
import { NodeIdSignal } from "../ilp-connector/computed/node-id"
import { peerProtocol as logger } from "../logger/instances"
import { ActiveSettlementSchemesSignal } from "../settlement-schemes/signals/active-settlement-schemes"
import { compareSetToArray } from "../utils/compare-sets"
import { PeersSignal } from "./computed/peers"
import { LINK_STATE_MAX_UPDATE_INTERVAL } from "./constants/timings"
import { ModifyNodeTableActor } from "./modify-node-table"
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
    const oldLinkState = sig.use(NodeTableStore).read().get(nodeId)?.linkState

    if (
      !oldLinkState ||
      !compareSetToArray(peers, oldLinkState.neighbors ?? []) ||
      !compareSetToArray(
        settlementSchemes,
        oldLinkState.settlementSchemes ?? [],
      ) ||
      oldLinkState.sequence <
        BigInt(Date.now()) - LINK_STATE_MAX_UPDATE_INTERVAL
    ) {
      // Sequence is the current time in milliseconds since 1970 but must be
      // greater than the previous sequence number
      const sequence = bigIntMax(
        BigInt(Date.now()),
        (oldLinkState?.sequence ?? 0n) + 1n,
      )
      const peerIds = [...peers]
      const settlementSchemeIds = [...settlementSchemes]

      const peerInfoEntries = [
        ...peerIds.map((nodeId) => ({
          type: "neighbor" as const,
          value: { nodeId },
        })),
        ...settlementSchemeIds.map((settlementSchemeId) => ({
          type: "settlementScheme" as const,
          value: { settlementSchemeId },
        })),
      ]

      const peerNodeInfoResult = peerNodeInfo.serialize({
        nodeId,
        publicKey: nodePublicKey,
        url,
        alias,
        sequence,
        entries: peerInfoEntries,
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
        signature: {
          type: "ed25519",
          value: signature,
        },
      })

      if (!message.success) {
        logger.warn("Failed to serialize link state update message", {
          error: message.error,
        })
        return
      }

      logger.debug(
        oldLinkState
          ? "updating own node table entry"
          : "creating own node table entry",
        {
          sequence,
          neighbors: peerIds.join(","),
        },
      )
      const modifyNodeTableActor = sig.use(ModifyNodeTableActor)
      if (oldLinkState === undefined) {
        modifyNodeTableActor.tell("addNode", nodeId)
      }

      modifyNodeTableActor.tell("processLinkState", {
        linkStateBytes: message.value,
        linkState: {
          nodeId,
          sequence,
          publicKey: nodePublicKey,
          url,
          alias,
          entries: peerInfoEntries,
        },
        retransmit: "immediately",
        from: nodeId,
      })
    }
  })
