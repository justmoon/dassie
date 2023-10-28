import { Reactor, createActor } from "@dassie/lib-reactive"

import { NodeIdSignal } from "../ilp-connector/computed/node-id"
import { peerProtocol as logger } from "../logger/instances"
import { SendPeerMessageActor } from "./actors/send-peer-message"
import { ModifyNodeTableActor } from "./modify-node-table"
import { NodeTableStore } from "./stores/node-table"
import { NodeId } from "./types/node-id"

const NODE_REFRESH_INTERVAL = 1000

/**
 * Minimum age in milliseconds for us to consider asking for an updated link state.
 */
const MINIMUM_AGE = 60_000n

export const RefreshNodeStateActor = (reactor: Reactor) => {
  const queryLinkState = async (nodeId: NodeId) => {
    const response = await reactor.use(SendPeerMessageActor).api.send.ask({
      destination: nodeId,
      message: {
        type: "linkStateRequest",
        value: {
          nodeId,
        },
      },
    })

    return response
  }

  return createActor((sig) => {
    const nodeTable = sig.use(NodeTableStore)
    const ownNodeId = sig.get(NodeIdSignal)

    const refreshNodeTick = async () => {
      try {
        await refreshNode()
      } catch (error) {
        logger.error("node link state refresh failed", { error })
      }
      sig.timeout(refreshNodeTick, NODE_REFRESH_INTERVAL)
    }

    const refreshNode = async () => {
      const oldNodes = [...nodeTable.read().entries()].filter(
        ([, node]) =>
          node.nodeId !== ownNodeId &&
          node.linkState != null &&
          node.linkState.sequence < BigInt(Date.now()) - MINIMUM_AGE,
      )

      if (oldNodes.length === 0) return

      const [nodeId] = oldNodes[Math.floor(Math.random() * oldNodes.length)]!

      // Send a link state request
      const linkState = await queryLinkState(nodeId)

      if (!linkState) return

      const { signed } = linkState.value

      reactor.use(ModifyNodeTableActor).api.processLinkState.tell({
        linkStateBytes: linkState.bytes,
        linkState: signed,
        retransmit: "never",
        from: nodeId,
      })
    }

    void refreshNodeTick()
  })
}
