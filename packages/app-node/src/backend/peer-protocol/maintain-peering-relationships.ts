import { Reactor, createActor } from "@dassie/lib-reactive"

import { NodeIdSignal } from "../ilp-connector/computed/node-id"
import { peerProtocol as logger } from "../logger/instances"
import { ActiveSettlementSchemesSignal } from "../settlement-schemes/signals/active-settlement-schemes"
import { PeersSignal } from "./computed/peers"
import { NodeTableStore } from "./stores/node-table"
import { SettlementSchemeId } from "./types/settlement-scheme-id"

const PEERING_CHECK_INTERVAL = 1000

const MINIMUM_PEERS = 2

function findCommonElement<T>(array: readonly T[], set: Set<T>): T | false {
  return array.find((element) => set.has(element)) ?? false
}

export const MaintainPeeringRelationshipsActor = (reactor: Reactor) =>
  createActor((sig) => {
    const ownNodeId = sig.get(NodeIdSignal)
    const ourSubnets = sig.get(ActiveSettlementSchemesSignal)

    const checkPeers = () => {
      try {
        const peersSet = sig.use(PeersSignal).read()
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
      const candidates = [...sig.use(NodeTableStore).read().values()]
        .map((candidate) => ({
          ...candidate,
          commonSettlementScheme: findCommonElement(
            candidate.linkState.settlementSchemes,
            ourSubnets,
          ),
        }))
        .filter(
          (
            node,
          ): node is typeof node & {
            commonSettlementScheme: SettlementSchemeId
          } =>
            node.nodeId !== ownNodeId &&
            node.peerState.id === "none" &&
            node.commonSettlementScheme !== false,
        )

      const randomNode =
        candidates[Math.floor(Math.random() * candidates.length)]

      if (!randomNode) {
        logger.debug("no valid candidates")
        return
      }

      reactor.use(NodeTableStore).updateNode(randomNode.nodeId, {
        peerState: {
          id: "request-peering",
          lastSeen: Date.now(),
          settlementSchemeId: randomNode.commonSettlementScheme,
        },
      })
    }

    checkPeers()
  })
