import { createLogger } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

import { EMPTY_UINT8ARRAY } from "../../../common/constants/general"
import type { IncomingPeerMessageEvent } from "../actions/handle-peer-message"
import { type NodeTableKey, nodeTableStore } from "../stores/node-table"

const logger = createLogger("das:node:handle-link-state-update")

export const MAX_LINK_STATE_UPDATE_RETRANSMIT_DELAY = 500

export const handleLinkStateUpdate = () =>
  createActor((sig) => {
    const nodeTable = sig.use(nodeTableStore)

    return {
      handle: ({
        message: {
          subnetId,
          content: {
            value: { value: content },
          },
        },
      }: IncomingPeerMessageEvent<"linkStateUpdate">) => {
        const { value: linkState, bytes: linkStateBytes } = content
        const { nodeId, url, sequence, entries, nodePublicKey } =
          linkState.signed
        const nodes = nodeTable.read()

        const neighbors = entries
          // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition
          .filter(({ type }) => type === "neighbor")
          .map((entry) => entry.value.nodeId)

        const nodeKey: NodeTableKey = `${subnetId}.${nodeId}`
        const node = nodes.get(nodeKey)
        const isPeer = node?.peerState.id !== "none"

        if (node) {
          if (sequence < node.linkState.sequence) {
            logger.debug("received a stale link state update", {
              subnet: subnetId,
              from: nodeId,
              sequence,
              neighbors: neighbors.join(","),
              previousSequence: node.linkState.sequence,
            })
            return EMPTY_UINT8ARRAY
          }

          if (sequence === node.linkState.sequence) {
            if (isPeer) {
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
                counter: node.linkState.updateReceivedCounter + 1,
              })
            }

            nodeTable.updateNode(nodeKey, {
              linkState: {
                ...node.linkState,
                updateReceivedCounter: node.linkState.updateReceivedCounter + 1,
              },
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
            linkState: {
              sequence,
              neighbors,
              lastUpdate: linkStateBytes,
              updateReceivedCounter: 1,
              scheduledRetransmitTime:
                Date.now() +
                Math.ceil(
                  Math.random() * MAX_LINK_STATE_UPDATE_RETRANSMIT_DELAY
                ),
            },
            peerState:
              node.peerState.id === "request-peering" ||
              node.peerState.id === "peered"
                ? { id: "peered", lastSeen: Date.now() }
                : node.peerState,
          })
        } else {
          nodeTable.addNode({
            subnetId,
            nodeId,
            url,
            nodePublicKey,
            linkState: {
              sequence,
              updateReceivedCounter: 1,
              scheduledRetransmitTime:
                Date.now() +
                Math.ceil(
                  Math.random() * MAX_LINK_STATE_UPDATE_RETRANSMIT_DELAY
                ),
              neighbors,
              lastUpdate: linkStateBytes,
            },
            peerState: { id: "none" },
          })
        }

        return EMPTY_UINT8ARRAY
      },
    }
  })
