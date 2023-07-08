import { createLogger } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

import { EMPTY_UINT8ARRAY } from "../../../common/constants/general"
import type { IncomingPeerMessageEvent } from "../actors/handle-peer-message"
import { nodeTableStore } from "../stores/node-table"
import { parseLinkStateEntries } from "../utils/parse-link-state-entries"

const logger = createLogger("das:node:handle-link-state-update")

export const MAX_LINK_STATE_UPDATE_RETRANSMIT_DELAY = 500

export const handleLinkStateUpdate = () =>
  createActor((sig) => {
    const nodeTable = sig.use(nodeTableStore)

    return {
      handle: ({
        message: {
          content: {
            value: { value: content },
          },
        },
      }: IncomingPeerMessageEvent<"linkStateUpdate">) => {
        const { value: linkState, bytes: linkStateBytes } = content
        const { nodeId, url, alias, sequence, entries, nodePublicKey } =
          linkState.signed
        const nodes = nodeTable.read()

        const { neighbors, settlementSchemes } = parseLinkStateEntries(entries)

        const node = nodes.get(nodeId)
        const isPeer = node?.peerState.id !== "none"

        if (node) {
          if (sequence < node.linkState.sequence) {
            logger.debug("received a stale link state update", {
              from: nodeId,
              sequence,
              neighbors: neighbors.join(","),
              settlementSchemes: settlementSchemes.join(","),
              previousSequence: node.linkState.sequence,
            })
            return EMPTY_UINT8ARRAY
          }

          if (sequence === node.linkState.sequence) {
            if (isPeer) {
              logger.debug("received heartbeat from peer", {
                from: nodeId,
                sequence,
              })
            } else {
              logger.debug("received another copy of a link state update", {
                from: nodeId,
                sequence,
                counter: node.linkState.updateReceivedCounter + 1,
              })
            }

            nodeTable.updateNode(nodeId, {
              linkState: {
                ...node.linkState,
                updateReceivedCounter: node.linkState.updateReceivedCounter + 1,
              },
            })
            return EMPTY_UINT8ARRAY
          }

          logger.debug("process new link state update", {
            from: nodeId,
            sequence,
            neighbors: neighbors.join(","),
          })
          nodeTable.updateNode(nodeId, {
            linkState: {
              sequence,
              neighbors,
              settlementSchemes,
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
                ? { ...node.peerState, id: "peered", lastSeen: Date.now() }
                : node.peerState,
          })
        } else {
          nodeTable.addNode({
            nodeId,
            url,
            alias,
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
              settlementSchemes,
              lastUpdate: linkStateBytes,
            },
            peerState: { id: "none" },
          })
        }

        return EMPTY_UINT8ARRAY
      },
    }
  })
