import { createLogger } from "@dassie/lib-logger"
import type { EffectContext } from "@dassie/lib-reactive"

import { EMPTY_UINT8ARRAY } from "../../../common/constants/general"
import type {
  IncomingPeerMessageEvent,
  PeerMessageContent,
} from "../actions/handle-peer-message"
import { NodeTableKey, nodeTableStore } from "../stores/node-table"
import { peerTableStore } from "../stores/peer-table"

const logger = createLogger("das:node:handle-link-state-update")

export const MAX_LINK_STATE_UPDATE_RETRANSMIT_DELAY = 500

export const handleLinkStateUpdate = (sig: EffectContext) => {
  const nodeTable = sig.use(nodeTableStore)
  const peerTable = sig.use(peerTableStore)

  return (
    content: PeerMessageContent<"linkStateUpdate">,
    { message: { subnetId } }: IncomingPeerMessageEvent
  ) => {
    const { value: linkState, bytes: linkStateBytes } = content
    const { nodeId, url, sequence, entries, nodePublicKey } = linkState.signed
    const nodes = nodeTable.read()
    const peers = peerTable.read()

    const neighbors = entries
      // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition
      .filter(({ type }) => type === "neighbor")
      .map((entry) => entry.value.nodeId)

    const nodeKey: NodeTableKey = `${subnetId}.${nodeId}`
    const node = nodes.get(nodeKey)
    const peer = peers.get(nodeKey)

    if (node) {
      if (sequence < node.sequence) {
        logger.debug("received a stale link state update", {
          subnet: subnetId,
          from: nodeId,
          sequence,
          neighbors: neighbors.join(","),
          previousSequence: node.sequence,
        })
        return EMPTY_UINT8ARRAY
      }

      if (sequence === node.sequence) {
        if (peer) {
          logger.debug("received heartbeat from peer", {
            subnet: subnetId,
            from: nodeId,
            sequence,
          })
        } else {
          logger.debug("received another copy of a link state update", {
            subnet: subnetId,
            from: nodeId,
            sequence,
            counter: node.updateReceivedCounter + 1,
          })
        }

        nodeTable.updateNode(nodeKey, {
          updateReceivedCounter: node.updateReceivedCounter + 1,
        })
        return EMPTY_UINT8ARRAY
      }

      logger.debug("process new link state update", {
        subnet: subnetId,
        from: nodeId,
        sequence,
        neighbors: neighbors.join(","),
      })
      nodeTable.updateNode(nodeKey, {
        sequence,
        neighbors,
        lastLinkStateUpdate: linkStateBytes,
        updateReceivedCounter: 1,
        scheduledRetransmitTime:
          Date.now() +
          Math.ceil(Math.random() * MAX_LINK_STATE_UPDATE_RETRANSMIT_DELAY),
      })
    } else {
      nodeTable.addNode({
        subnetId,
        nodeId,
        url,
        nodePublicKey,
        sequence,
        updateReceivedCounter: 1,
        scheduledRetransmitTime:
          Date.now() +
          Math.ceil(Math.random() * MAX_LINK_STATE_UPDATE_RETRANSMIT_DELAY),
        neighbors,
        lastLinkStateUpdate: linkStateBytes,
      })
    }

    if (peer) {
      // If this peer has started sending us node updates, we can infer that we are successfully peered
      if (peer.state.id === "request-peering") {
        peerTable.updatePeer(nodeKey, {
          state: { id: "peered" },
        })
      }

      peerTable.updatePeer(nodeKey, {
        lastSeen: Date.now(),
      })
    }

    return EMPTY_UINT8ARRAY
  }
}
