import { Reactor, createActor } from "@dassie/lib-reactive"

import { DassieActorContext } from "../base/types/dassie-base"
import { EnvironmentConfigSignal } from "../config/environment-config"
import { NodeIdSignal } from "../ilp-connector/computed/node-id"
import { peerProtocol as logger } from "../logger/instances"
import { NODE_STATE_STALE_TIMEOUT } from "./constants/timings"
import { ModifyNodeTable } from "./functions/modify-node-table"
import { SendPeerMessage } from "./functions/send-peer-message"
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
  const nodeTableStore = reactor.use(NodeTableStore)
  const modifyNodeTable = reactor.use(ModifyNodeTable)
  const nodeIdSignal = reactor.use(NodeIdSignal)
  const sendPeerMessage = reactor.use(SendPeerMessage)

  const queryLinkState = async (
    oracleNodeId: NodeId,
    subjectNodeId: NodeId,
  ) => {
    const response = await sendPeerMessage({
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

  return createActor(async (sig: DassieActorContext) => {
    const bootstrapNodes = sig
      .readAndTrack(EnvironmentConfigSignal, (config) => config.bootstrapNodes)
      .map(({ id }) => id)

    logger.assert(
      bootstrapNodes.length > 0,
      "expected at least one bootstrap node",
    )

    async function refreshNode() {
      const ownNodeId = nodeIdSignal.read()
      const oldestAcceptableSequence = BigInt(
        Date.now() - NODE_STATE_STALE_TIMEOUT,
      )
      const currentNodes: NodeId[] = []
      const oldNodes: NodeId[] = []

      for (const node of nodeTableStore.read().values()) {
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
        !nodeTableStore.read().get(nodeId)?.linkState ||
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

      modifyNodeTable.processLinkState({
        linkStateBytes: linkState.bytes,
        linkState: signed,
        retransmit: "never",
      })
    }

    const task = sig.task({
      handler: refreshNode,
      interval: NODE_REFRESH_INTERVAL,
    })
    await task.execute()
    task.schedule()
  })
}
