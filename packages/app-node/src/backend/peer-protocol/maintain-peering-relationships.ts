import { createLogger } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

import { configSignal } from "../config"
import { nodeIdSignal } from "../ilp-connector/computed/node-id"
import { peersComputation } from "./computed/peers"
import type { PerSubnetParameters } from "./run-per-subnet-effects"
import { nodeTableStore } from "./stores/node-table"

const PEERING_CHECK_INTERVAL = 1000

const MINIMUM_PEERS = 2

const logger = createLogger(
  "das:app-node:peer-protocol:maintain-peering-relationships"
)

export const maintainPeeringRelationships = () =>
  createActor((sig, { subnetId }: PerSubnetParameters) => {
    const subnetConfig = sig.get(configSignal, (state) =>
      state.initialSubnets.find(({ id }) => id === subnetId)
    )
    const ownNodeId = sig.get(nodeIdSignal)

    if (!subnetConfig) {
      throw new Error(`Subnet '${subnetId}' is not configured`)
    }

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

      sig.timeout(() => checkPeers(), PEERING_CHECK_INTERVAL)
    }

    const addPeer = () => {
      // TODO: This is slow but once we switch node table to an sqlite table, we'll be able to optimize it
      const candidates = [...sig.use(nodeTableStore).read().values()].filter(
        (node) => {
          return node.nodeId !== ownNodeId && node.peerState.id === "none"
        }
      )

      const randomNode =
        candidates[Math.floor(Math.random() * candidates.length)]

      if (!randomNode) {
        logger.debug("no valid candidates")
        return
      }

      reactor
        .use(nodeTableStore)
        .updateNode(`${subnetId}.${randomNode.nodeId}`, {
          peerState: { id: "request-peering", lastSeen: Date.now() },
        })
    }

    checkPeers()
  })
