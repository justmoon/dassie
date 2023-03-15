import { createLogger } from "@dassie/lib-logger"

import type {
  IncomingPeerMessageHandlerParameters,
  PeerMessageContent,
} from "../handle-peer-message"
import { NodeTableKey, nodeTableStore } from "../stores/node-table"
import { peerTableStore } from "../stores/peer-table"

const logger = createLogger("das:node:handle-link-state-update")

export const MAX_LINK_STATE_UPDATE_RETRANSMIT_DELAY = 500

export const handleLinkStateUpdate = (
  content: PeerMessageContent<"linkStateUpdate">,
  { message: { subnetId }, reactor }: IncomingPeerMessageHandlerParameters
) => {
  const { value: linkState, bytes: linkStateBytes } = content
  const { nodeId, sequence, entries, nodePublicKey } = linkState.signed
  const nodes = reactor.use(nodeTableStore).read()
  const peers = reactor.use(peerTableStore).read()

  const neighbors = entries
    .filter((entry) => "neighbor" in entry)
    .map((entry) => entry.neighbor.nodeId)

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
      return
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

      reactor.use(nodeTableStore).updateNode(nodeKey, {
        updateReceivedCounter: node.updateReceivedCounter + 1,
      })
      return
    }

    logger.debug("process new link state update", {
      subnet: subnetId,
      from: nodeId,
      sequence,
      neighbors: neighbors.join(","),
    })
    reactor.use(nodeTableStore).updateNode(nodeKey, {
      sequence,
      neighbors,
      lastLinkStateUpdate: linkStateBytes,
      updateReceivedCounter: 1,
      scheduledRetransmitTime:
        Date.now() +
        Math.ceil(Math.random() * MAX_LINK_STATE_UPDATE_RETRANSMIT_DELAY),
    })
  } else {
    reactor.use(nodeTableStore).addNode({
      subnetId,
      nodeId,
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
      reactor.use(peerTableStore).updatePeer(nodeKey, {
        state: { id: "peered" },
      })
    }

    reactor.use(peerTableStore).updatePeer(nodeKey, {
      lastSeen: Date.now(),
    })
  }
}
