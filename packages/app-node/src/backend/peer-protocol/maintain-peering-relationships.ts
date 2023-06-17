import { createLogger } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

import { nodeIdSignal } from "../ilp-connector/computed/node-id"
import { activeSubnetsSignal } from "../subnets/signals/active-subnets"
import { peersComputation } from "./computed/peers"
import { nodeTableStore } from "./stores/node-table"
import { SubnetId } from "./types/subnet-id"

const PEERING_CHECK_INTERVAL = 1000

const MINIMUM_PEERS = 2

const logger = createLogger(
  "das:app-node:peer-protocol:maintain-peering-relationships"
)

function findCommonElement<T>(array: T[], set: Set<T>): T | false {
  return array.find((element) => set.has(element)) ?? false
}

export const maintainPeeringRelationships = () =>
  createActor((sig) => {
    const ownNodeId = sig.get(nodeIdSignal)
    const ourSubnets = sig.get(activeSubnetsSignal)

    const { reactor } = sig
    const checkPeers = () => {
      try {
        const peersSet = sig.use(peersComputation).read()
        if (peersSet.size >= MINIMUM_PEERS) {
          return
        }

        addPeer()
      } catch (error) {
        logger.error("peer check failed", { error })
      }

      sig.timeout(checkPeers, PEERING_CHECK_INTERVAL)
    }

    const addPeer = () => {
      // TODO: This is slow but once we switch node table to an sqlite table, we'll be able to optimize it
      const candidates = [...sig.use(nodeTableStore).read().values()]
        .map((candidate) => ({
          ...candidate,
          commonSubnet: findCommonElement(
            candidate.linkState.subnets,
            ourSubnets
          ),
        }))
        .filter(
          (node): node is typeof node & { commonSubnet: SubnetId } =>
            node.nodeId !== ownNodeId &&
            node.peerState.id === "none" &&
            node.commonSubnet !== false
        )

      const randomNode =
        candidates[Math.floor(Math.random() * candidates.length)]

      if (!randomNode) {
        logger.debug("no valid candidates")
        return
      }

      reactor.use(nodeTableStore).updateNode(randomNode.nodeId, {
        peerState: {
          id: "request-peering",
          lastSeen: Date.now(),
          subnetId: randomNode.commonSubnet,
        },
      })
    }

    checkPeers()
  })
