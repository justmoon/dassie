import { Reactor, createActor } from "@dassie/lib-reactive"

import { peerProtocol as logger } from "../logger/instances"
import { SendPeerMessageActor } from "./actors/send-peer-message"
import { signedPeerNodeInfo } from "./peer-schema"
import { NodeDiscoveryQueueStore } from "./stores/node-discovery-queue"
import { NodeTableStore } from "./stores/node-table"
import { NodeId } from "./types/node-id"
import { parseLinkStateEntries } from "./utils/parse-link-state-entries"

const NODE_DISCOVERY_INTERVAL = 1000

export const DiscoverNodesActor = (reactor: Reactor) => {
  const queryLinkState = async (
    oracleNodeId: NodeId,
    subjectNodeId: NodeId,
  ) => {
    const response = await reactor.use(SendPeerMessageActor).ask("send", {
      destination: oracleNodeId,
      message: {
        type: "linkStateRequest",
        value: {
          nodeId: subjectNodeId,
        },
      },
    })

    if (!response?.length) {
      throw new Error("no/invalid response")
    }

    return response
  }

  return createActor((sig) => {
    const nodeDiscoveryQueue = sig.use(NodeDiscoveryQueueStore)

    const discoverNodeTick = async () => {
      try {
        await discoverNode()
      } catch (error) {
        logger.error("node discovery failed", { error })
      }
      sig.timeout(discoverNodeTick, NODE_DISCOVERY_INTERVAL)
    }

    const discoverNode = async () => {
      const [nextNode] = nodeDiscoveryQueue.read()

      if (!nextNode) return

      const [nodeId, referrerNodeId] = nextNode

      logger.debug("discovering node", { nodeId, referrerNodeId })

      nodeDiscoveryQueue.removeNode(nodeId)

      // Send a peer discovery request
      const linkState = await queryLinkState(referrerNodeId, nodeId)

      const { signed } = signedPeerNodeInfo.parseOrThrow(linkState)

      const { neighbors, settlementSchemes } = parseLinkStateEntries(
        signed.entries,
      )

      const nodeTable = sig.use(NodeTableStore)

      const node = nodeTable.read().get(nodeId)
      if (node) {
        if (node.linkState.sequence < signed.sequence) {
          nodeTable.updateNode(nodeId, {
            linkState: {
              sequence: signed.sequence,
              lastUpdate: linkState,
              updateReceivedCounter: 0,
              scheduledRetransmitTime: 0,
              neighbors,
              settlementSchemes,
            },
          })
        }
      } else {
        nodeTable.addNode({
          nodeId,
          url: signed.url,
          alias: signed.alias,
          nodePublicKey: signed.nodePublicKey,
          linkState: {
            sequence: signed.sequence,
            lastUpdate: linkState,
            updateReceivedCounter: 0,
            scheduledRetransmitTime: 0,
            neighbors,
            settlementSchemes,
          },
          peerState: { id: "none" },
        })
      }
    }

    void discoverNodeTick()
  })
}
