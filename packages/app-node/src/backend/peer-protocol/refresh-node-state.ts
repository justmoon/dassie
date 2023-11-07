import assert from "node:assert"

import { Reactor, createActor } from "@dassie/lib-reactive"

import { EnvironmentConfigSignal } from "../config/environment-config"
import { NodeIdSignal } from "../ilp-connector/computed/node-id"
import { peerProtocol as logger } from "../logger/instances"
import { SendPeerMessageActor } from "./actors/send-peer-message"
import { NODE_STATE_STALE_TIMEOUT } from "./constants/timings"
import { ModifyNodeTableActor } from "./modify-node-table"
import { NodeTableStore } from "./stores/node-table"
import { NodeId } from "./types/node-id"

const NODE_REFRESH_INTERVAL = 1000

/**
 * Chance that we will query a node's status indirectly, even if we have an old link state.
 *
 * This will allow us to eventually recover if we have an old, no longer valid URL for a node.
 */
const INDIRECT_QUERY_PROBABILITY = 0.2

export const RefreshNodeStateActor = (reactor: Reactor) => {
  const queryLinkState = async (
    oracleNodeId: NodeId,
    subjectNodeId: NodeId,
  ) => {
    const response = await reactor.use(SendPeerMessageActor).api.send.ask({
      destination: oracleNodeId,
      message: {
        type: "linkStateRequest",
        value: {
          nodeIds: [subjectNodeId],
        },
      },
    })

    return response?.[0]?.type === "found" ? response[0].value : undefined
  }

  return createActor((sig) => {
    const nodeTable = sig.use(NodeTableStore)
    const ownNodeId = sig.get(NodeIdSignal)
    const bootstrapNodes = sig
      .get(EnvironmentConfigSignal, (config) => config.bootstrapNodes)
      .map(({ id }) => id)

    assert(bootstrapNodes.length > 0)

    const refreshNodeTick = async () => {
      try {
        await refreshNode()
      } catch (error) {
        logger.error("node link state refresh failed", { error })
      }
      sig.timeout(refreshNodeTick, NODE_REFRESH_INTERVAL)
    }

    const refreshNode = async () => {
      const oldestAcceptableSequence = BigInt(
        Date.now() - NODE_STATE_STALE_TIMEOUT,
      )
      const currentNodes: NodeId[] = []
      const oldNodes: NodeId[] = []

      for (const node of nodeTable.read().values()) {
        if (node.nodeId === ownNodeId) continue

        if (
          node.linkState != null &&
          node.linkState.sequence > oldestAcceptableSequence
        ) {
          currentNodes.push(node.nodeId)
        } else {
          oldNodes.push(node.nodeId)
        }
      }

      if (oldNodes.length === 0) return

      const nodeId = oldNodes[Math.floor(Math.random() * oldNodes.length)]!

      let oracleNodeId = nodeId
      if (
        !nodeTable.read().get(nodeId)?.linkState ||
        Math.random() < INDIRECT_QUERY_PROBABILITY
      ) {
        const queryableNodes = [...currentNodes, ...bootstrapNodes]
        oracleNodeId =
          queryableNodes[Math.floor(Math.random() * queryableNodes.length)]!
      }

      // Send a link state request
      const linkState = await queryLinkState(oracleNodeId, nodeId)

      if (!linkState) return

      const { signed } = linkState.value

      reactor.use(ModifyNodeTableActor).api.processLinkState.tell({
        linkStateBytes: linkState.bytes,
        linkState: signed,
        retransmit: "never",
      })
    }

    void refreshNodeTick()
  })
}
