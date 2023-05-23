import { setTimeout } from "node:timers/promises"

import { createLogger } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

import { sendPeerMessage } from "./actors/send-peer-message"
import { signedPeerNodeInfo } from "./peer-schema"
import { nodeDiscoveryQueueStore } from "./stores/node-discovery-queue"
import { nodeTableStore } from "./stores/node-table"
import { parseLinkStateEntries } from "./utils/parse-link-state-entries"

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

      const [nodeId, referrerNodeId] = nextNode

      logger.debug("discovering node", { nodeId, referrerNodeId })

      nodeDiscoveryQueue.removeNode(nodeId)

      // Send a peer discovery request
      const linkState = await queryLinkState(referrerNodeId, nodeId)

      const { signed } = signedPeerNodeInfo.parseOrThrow(linkState)

      const { neighbors, subnets } = parseLinkStateEntries(signed.entries)

      const nodeTable = sig.use(nodeTableStore)

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
              subnets,
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
            subnets,
          },
          peerState: { id: "none" },
        })
      }
    }

    const queryLinkState = async (
      oracleNodeId: string,
      subjectNodeId: string
    ) => {
      const response = await reactor.use(sendPeerMessage).ask("send", {
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

    void discoverPeerLoop()
  })
