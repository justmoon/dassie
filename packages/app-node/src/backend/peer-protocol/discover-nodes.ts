import { Infer } from "@dassie/lib-oer"
import { Reactor, createActor } from "@dassie/lib-reactive"

import { EnvironmentConfigSignal } from "../config/environment-config"
import { NodeIdSignal } from "../ilp-connector/computed/node-id"
import { peerProtocol as logger } from "../logger/instances"
import { SendPeerMessageActor } from "./actors/send-peer-message"
import { nodeListResponse } from "./peer-schema"
import { NodeTableStore } from "./stores/node-table"
import { NodeId } from "./types/node-id"
import { parseLinkStateEntries } from "./utils/parse-link-state-entries"

const NODE_DISCOVERY_INTERVAL = 10 * 1000

export const DiscoverNodesActor = (reactor: Reactor) => {
  const loadNodeList = async (
    sourceNodeId: NodeId,
  ): Promise<Infer<typeof nodeListResponse>> => {
    const response = await reactor.use(SendPeerMessageActor).ask("send", {
      destination: sourceNodeId,
      message: {
        type: "nodeListRequest",
        value: {},
      },
    })

    if (!response?.length) {
      logger.warn("no/invalid response to node list request", {
        from: sourceNodeId,
      })
      return []
    }

    const nodeList = nodeListResponse.parseOrThrow(response)

    return nodeList
  }

  const registerWithNode = (targetNodeId: NodeId, ourNodeInfo: Uint8Array) => {
    reactor.use(SendPeerMessageActor).tell("send", {
      destination: targetNodeId,
      message: {
        type: "registration",
        value: {
          nodeInfo: {
            bytes: ourNodeInfo,
          },
        },
      },
    })
  }

  return createActor((sig) => {
    const environmentConfig = sig.use(EnvironmentConfigSignal)
    const nodeTable = sig.use(NodeTableStore)
    const ownNodeId = sig.get(NodeIdSignal)

    const discoverNodesTick = async () => {
      try {
        await discoverNodes()
      } catch (error) {
        logger.error("node discovery failed", { error })
      }
      sig.timeout(discoverNodesTick, NODE_DISCOVERY_INTERVAL)
    }

    const discoverNodes = async () => {
      const { bootstrapNodes } = environmentConfig.read()

      const bootstrapNodeIds = bootstrapNodes.map((node) => node.id)

      const promises = bootstrapNodeIds.map((nodeId) => loadNodeList(nodeId))

      const nodeLists = await Promise.all(promises)

      const nodeListCount = nodeLists.length

      const latestNodeInfoMap = new Map<
        NodeId,
        (typeof nodeLists)[number][number]
      >()
      const nodeAppearanceCountMap = new Map<NodeId, number>()

      const bootstrapNodesWhoDontKnowUs = new Set<NodeId>()

      for (const [bootstrapNodeIndex, nodeList] of nodeLists.entries()) {
        let foundOurselves = false
        for (const node of nodeList) {
          const {
            value: {
              signed: { nodeId, sequence },
            },
          } = node

          if (nodeId === ownNodeId) {
            foundOurselves = true
          }

          nodeAppearanceCountMap.set(
            nodeId,
            (nodeAppearanceCountMap.get(nodeId) ?? 0) + 1,
          )

          const latestNode = latestNodeInfoMap.get(nodeId)

          if (!latestNode || latestNode.value.signed.sequence < sequence) {
            latestNodeInfoMap.set(nodeId, node)
          }
        }

        if (!foundOurselves) {
          bootstrapNodesWhoDontKnowUs.add(bootstrapNodeIds[bootstrapNodeIndex]!)
        }
      }

      for (const [nodeId, node] of latestNodeInfoMap) {
        const nodeCount = nodeAppearanceCountMap.get(nodeId) ?? 0
        if (
          bootstrapNodeIds.includes(nodeId) ||
          nodeCount > nodeListCount / 2
        ) {
          const {
            value: { signed },
            bytes: linkState,
          } = node

          const { neighbors, settlementSchemes } = parseLinkStateEntries(
            signed.entries,
          )

          if (!nodeTable.read().has(nodeId)) {
            logger.debug(
              bootstrapNodeIds.includes(nodeId)
                ? "adding bootstrap node to node table"
                : "adding node by majority of bootstrap nodes",
              {
                nodeId,
                nodeCount,
              },
            )
            nodeTable.addNode({
              nodeId,
              linkState: {
                lastUpdate: linkState,
                sequence: signed.sequence,
                publicKey: signed.publicKey,
                url: signed.url,
                alias: signed.alias,
                updateReceivedCounter: 0,
                scheduledRetransmitTime: 0,
                neighbors,
                settlementSchemes,
              },
              peerState: { id: "none" },
            })
          }
        }
      }

      for (const bootstrapNodeId of bootstrapNodesWhoDontKnowUs) {
        const ourNodeInfo = nodeTable.read().get(ownNodeId)?.linkState
          ?.lastUpdate

        if (ourNodeInfo) {
          registerWithNode(bootstrapNodeId, ourNodeInfo)
        }
      }
    }

    void discoverNodesTick()
  })
}
