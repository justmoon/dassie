import Denque from "denque"

import { createLogger } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"
import { assertDefined } from "@dassie/lib-type-utils"

import {
  processPacketPrepare,
  processPacketResult,
} from "../accounting/functions/process-interledger-packet"
import { ledgerStore } from "../accounting/stores/ledger"
import { configSignal } from "../config"
import { nodeIdSignal } from "../ilp-connector/computed/node-id"
import { IlpType } from "../ilp-connector/ilp-packet-codec"
import { globalIlpRoutingTableSignal } from "../ilp-connector/signals/global-ilp-routing-table"
import { sendPeerMessage } from "./actors/send-peer-message"
import { nodeDiscoveryQueueStore } from "./stores/node-discovery-queue"
import { nodeTableStore } from "./stores/node-table"
import {
  type RoutingTableEntry,
  routingTableStore,
} from "./stores/routing-table"
import { NodeId } from "./types/node-id"

const logger = createLogger("das:node:calculate-routes")

interface NodeInfoEntry {
  level: number
  parents: NodeId[]
}

/**
 * This actor generates an Even-Shiloach tree which is then condensed into a routing table. The routing table contains all possible first hops which are on one of the shortest paths to the target node.
 */
export const calculateRoutes = () =>
  createActor((sig) => {
    const { ilpAllocationScheme } = sig.getKeys(configSignal, [
      "ilpAllocationScheme",
    ])
    const ownNodeId = sig.get(nodeIdSignal)
    const nodeTable = sig.get(nodeTableStore)

    const ownNodeTableEntry = nodeTable.get(ownNodeId)

    const queue = new Denque([ownNodeTableEntry])
    const queueSet = new Set([ownNodeId])

    const nodeInfoMap = new Map<NodeId, NodeInfoEntry>()
    nodeInfoMap.set(ownNodeId, { level: 0, parents: [] })

    let currentNode
    while ((currentNode = queue.shift()) != undefined) {
      queueSet.delete(currentNode.nodeId)

      const currentNodeInfo = nodeInfoMap.get(currentNode.nodeId)
      assertDefined(currentNodeInfo)

      for (const neighborId of currentNode.linkState.neighbors) {
        const neighborInfo = nodeInfoMap.get(neighborId)

        if (neighborInfo) {
          if (neighborInfo.level > currentNodeInfo.level + 1) {
            // this is a new shortest path to this node
            neighborInfo.level = currentNodeInfo.level + 1
            neighborInfo.parents = [currentNode.nodeId]
          } else if (neighborInfo.level === currentNodeInfo.level + 1) {
            // this is an alternative path of equal length
            neighborInfo.parents.push(currentNode.nodeId)
          } else {
            // this is a longer path than the one(s) we already have, ignore
          }
        } else {
          // this is a node we haven't reached before
          const newNeighborInfo = {
            level: currentNodeInfo.level + 1,
            parents: [currentNode.nodeId],
          }
          nodeInfoMap.set(neighborId, newNeighborInfo)

          const neighbor = nodeTable.get(neighborId)

          if (neighbor) {
            queue.push(neighbor)
            queueSet.add(neighborId)
          } else {
            // We came across a node in the graph that we don't know. Let's ask someone about it.
            sig
              .use(nodeDiscoveryQueueStore)
              .addNode(neighborId, currentNode.nodeId)
          }
        }
      }
    }

    const routes = new Map<string, RoutingTableEntry>()
    const ilpRoutingTable = sig.get(globalIlpRoutingTableSignal)
    for (const [nodeId, nodeInfo] of nodeInfoMap.entries()) {
      let level = nodeInfo.level
      let parents = new Set(nodeInfo.parents)

      while (--level > 1) {
        parents = new Set(
          [...parents].flatMap(
            (parentId) => nodeInfoMap.get(parentId)?.parents ?? []
          )
        )
      }

      const firstHopOptions = nodeInfo.level === 1 ? [nodeId] : [...parents]

      routes.set(nodeId, {
        distance: nodeInfo.level,
        firstHopOptions,
      })

      if (nodeInfo.level > 0) {
        const ilpAddress = `${ilpAllocationScheme}.das.${nodeId}`

        // TODO: We shouldn't replicate this for every destination. The routing table should only store the peer ID and there should then be a routine for sending an ILP packet via a peer.
        ilpRoutingTable.set(ilpAddress, {
          prefix: ilpAddress,
          type: "peer",
          sendPreparePacket: ({
            parsedPacket,
            serializedPacket,
            outgoingRequestId: requestId,
          }) => {
            const nextHop = firstHopOptions[0]!

            const peerState = sig
              .use(nodeTableStore)
              .read()
              .get(nextHop)?.peerState

            if (peerState?.id !== "peered") {
              throw new Error(`Next hop node is not actually peered ${nextHop}`)
            }

            const ledger = sig.use(ledgerStore)
            if (parsedPacket.amount > 0n) {
              processPacketPrepare(
                ledger,
                `${peerState.subnetId}/peer/${nextHop}/interledger`,
                parsedPacket,
                "outgoing"
              )
            }

            logger.debug("sending ilp packet", { nextHop })

            sig.use(sendPeerMessage).tell("send", {
              destination: nextHop,
              message: {
                type: "interledgerPacket",
                value: {
                  signed: {
                    requestId: requestId,
                    packet: serializedPacket,
                  },
                },
              },
            })
          },
          sendResultPacket: ({
            parsedPacket: packet,
            serializedPacket: asUint8Array,
            prepare: {
              parsedPacket: preparePacket,
              incomingRequestId: requestId,
            },
          }) => {
            const nextHop = firstHopOptions[0]!

            const peerState = sig
              .use(nodeTableStore)
              .read()
              .get(nextHop)?.peerState

            if (peerState?.id !== "peered") {
              throw new Error(`Next hop node is not actually peered ${nextHop}`)
            }

            const ledger = sig.use(ledgerStore)
            if (preparePacket.amount > 0n) {
              processPacketResult(
                ledger,
                `${peerState.subnetId}/peer/${nextHop}/interledger`,
                preparePacket,
                packet.type === IlpType.Fulfill ? "fulfill" : "reject"
              )
            }

            logger.debug("sending ilp packet", { nextHop })

            sig.use(sendPeerMessage).tell("send", {
              destination: nextHop,
              message: {
                type: "interledgerPacket",
                value: {
                  signed: {
                    requestId: requestId,
                    packet: asUint8Array,
                  },
                },
              },
            })
          },
        })
      }
    }

    sig.onCleanup(() => {
      for (const nodeId of nodeInfoMap.keys()) {
        const ilpAddress = `${ilpAllocationScheme}.das.${nodeId}`
        ilpRoutingTable.delete(ilpAddress)
      }
    })

    sig.use(routingTableStore).write(routes)
  })
