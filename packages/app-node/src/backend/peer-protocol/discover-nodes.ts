import { setTimeout } from "node:timers/promises"

import { createLogger } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

import { sendPeerMessage } from "./actors/send-peer-message"
import { signedPeerNodeInfo } from "./peer-schema"
import { nodeDiscoveryQueueStore } from "./stores/node-discovery-queue"
import { nodeTableStore, parseNodeKey } from "./stores/node-table"

const NODE_DISCOVERY_INTERVAL = 1000

const logger = createLogger("das:app-node:peer-protocol:discover-nodes")

export const discoverNodes = () =>
  createActor((sig) => {
    const nodeDiscoveryQueue = sig.use(nodeDiscoveryQueueStore)

    const { reactor, lifecycle } = sig
    const discoverPeerLoop = async () => {
      for (;;) {
        try {
          if (lifecycle.isDisposed) return

          await discoverPeer()
        } catch (error) {
          logger.error("node discovery failed", { error })
        }
        await setTimeout(NODE_DISCOVERY_INTERVAL)
      }
    }

    const discoverPeer = async () => {
      const [nextNode] = nodeDiscoveryQueue.read()

      if (!nextNode) return

      const [nodeKey, referrerNodeId] = nextNode

      logger.debug("discovering node", { nodeKey, referrerNodeId })

      nodeDiscoveryQueue.removeNode(nodeKey)

      const [subnetId, nodeId] = parseNodeKey(nodeKey)

      // Send a peer discovery request
      const linkState = await queryLinkState(subnetId, referrerNodeId, nodeId)

      const { signed } = signedPeerNodeInfo.parseOrThrow(linkState)

      const nodeTable = sig.use(nodeTableStore)

      const node = nodeTable.read().get(nodeKey)
      if (node) {
        if (node.linkState.sequence < signed.sequence) {
          nodeTable.updateNode(nodeKey, {
            linkState: {
              sequence: signed.sequence,
              lastUpdate: linkState,
              updateReceivedCounter: 0,
              scheduledRetransmitTime: 0,
              neighbors: signed.entries.map(({ value: { nodeId } }) => nodeId),
            },
          })
        }
      } else {
        nodeTable.addNode({
          nodeId,
          subnetId,
          url: signed.url,
          alias: signed.alias,
          nodePublicKey: signed.nodePublicKey,
          linkState: {
            sequence: signed.sequence,
            lastUpdate: linkState,
            updateReceivedCounter: 0,
            scheduledRetransmitTime: 0,
            neighbors: signed.entries.map(({ value: { nodeId } }) => nodeId),
          },
          peerState: { id: "none" },
        })
      }
    }

    const queryLinkState = async (
      subnetId: string,
      oracleNodeId: string,
      subjectNodeId: string
    ) => {
      const response = await reactor.use(sendPeerMessage).ask("send", {
        subnet: subnetId,
        destination: oracleNodeId,
        message: {
          type: "linkStateRequest",
          value: {
            subnetId,
            nodeId: subjectNodeId,
          },
        },
      })

      if (!response?.length) {
        throw new Error("no/invalid response")
      }

      return response
    }

    void discoverPeerLoop()
  })
