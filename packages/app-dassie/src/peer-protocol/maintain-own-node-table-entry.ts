import { createActor } from "@dassie/lib-reactive"
import { bigIntMax, isFailure } from "@dassie/lib-type-utils"

import { DatabaseConfigStore } from "../config/database-config"
import { NodePublicKeySignal } from "../crypto/computed/node-public-key"
import { SignWithDassieKey } from "../crypto/functions/sign-with-dassie-key"
import { NodeIdSignal } from "../ilp-connector/computed/node-id"
import { ActiveSettlementSchemesSignal } from "../ledgers/signals/active-settlement-schemes"
import { peerProtocol as logger } from "../logger/instances"
import { compareSetToArray } from "../utils/compare-sets"
import { PeersSignal } from "./computed/peers"
import { LINK_STATE_MAX_UPDATE_INTERVAL } from "./constants/timings"
import { ModifyNodeTable } from "./functions/modify-node-table"
import { peerNodeInfo, signedPeerNodeInfo } from "./peer-schema"
import { NodeTableStore } from "./stores/node-table"

export const MaintainOwnNodeTableEntryActor = () =>
  createActor(async (sig) => {
    const signWithDassieKey = sig.reactor.use(SignWithDassieKey)

    // Get the current peers and re-run the actor if they change
    const peers = sig.readAndTrack(PeersSignal)
    const settlementSchemes = sig.readAndTrack(ActiveSettlementSchemesSignal)

    const nodeId = sig.readAndTrack(NodeIdSignal)
    const nodePublicKey = sig.readAndTrack(NodePublicKeySignal)
    const { url, alias } = sig.readAndTrack(DatabaseConfigStore)
    const oldLinkState = sig.read(NodeTableStore).get(nodeId)?.linkState

    if (
      !oldLinkState ||
      !compareSetToArray(peers, oldLinkState.neighbors) ||
      !compareSetToArray(settlementSchemes, oldLinkState.settlementSchemes) ||
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

      if (isFailure(peerNodeInfoResult)) {
        logger.warn("Failed to serialize link state update signed portion", {
          error: peerNodeInfoResult,
        })
        return
      }

      const signature = await signWithDassieKey(peerNodeInfoResult)
      const message = signedPeerNodeInfo.serialize({
        signed: { bytes: peerNodeInfoResult },
        signature: {
          type: "ed25519",
          value: signature,
        },
      })

      if (isFailure(message)) {
        logger.warn("Failed to serialize link state update message", {
          error: message,
        })
        return
      }

      logger.debug?.(
        oldLinkState ?
          "updating own node table entry"
        : "creating own node table entry",
        {
          sequence,
          neighbors: peerIds.join(","),
        },
      )
      const modifyNodeTable = sig.reactor.use(ModifyNodeTable)
      if (oldLinkState === undefined) {
        modifyNodeTable.addNode(nodeId)
      }

      modifyNodeTable.processLinkState({
        linkStateBytes: message,
        linkState: {
          nodeId,
          sequence,
          publicKey: nodePublicKey,
          url,
          alias,
          entries: peerInfoEntries,
        },
        retransmit: "immediately",
      })
    }
  })
