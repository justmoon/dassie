import { Reactor, createActor } from "@dassie/lib-reactive"

import { DassieActorContext } from "../base/types/dassie-base"
import { NodeIdSignal } from "../ilp-connector/computed/node-id"
import { peerProtocol as logger } from "../logger/instances"
import { ManageSettlementSchemeInstancesActor } from "../settlement-schemes/manage-settlement-scheme-instances"
import { ActiveSettlementSchemesSignal } from "../settlement-schemes/signals/active-settlement-schemes"
import { SendPeerMessageActor } from "./actors/send-peer-message"
import { PeersSignal } from "./computed/peers"
import { NodeTableStore } from "./stores/node-table"
import { SettlementSchemeId } from "./types/settlement-scheme-id"

const PEERING_CHECK_INTERVAL = 1000

const MINIMUM_PEERS = 2

function findCommonElement<T>(array: readonly T[], set: Set<T>): T | false {
  return array.find((element) => set.has(element)) ?? false
}

export const MaintainPeeringRelationshipsActor = (reactor: Reactor) => {
  const nodeTableStore = reactor.use(NodeTableStore)
  const nodeIdSignal = reactor.use(NodeIdSignal)
  const activeSettlementSchemesSignal = reactor.use(
    ActiveSettlementSchemesSignal,
  )
  const peersSignal = reactor.use(PeersSignal)

  async function addPeersIfNecessary() {
    const peersSet = peersSignal.read()
    if (peersSet.size >= MINIMUM_PEERS) {
      return
    }

    const ownNodeId = nodeIdSignal.read()
    const ourSubnets = activeSettlementSchemesSignal.read()

    // TODO: This is slow but we will optimize it later
    const candidates = [...nodeTableStore.read().values()]
      .filter(
        (node): node is typeof node & { linkState: object } => !!node.linkState,
      )
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

    const randomNode = candidates[Math.floor(Math.random() * candidates.length)]

    if (!randomNode) {
      return
    }

    const ownLinkState = nodeTableStore.read().get(ownNodeId)?.linkState

    if (!ownLinkState) {
      logger.warn("node table does not contain own link state")
      return
    }

    const settlementSchemeId = randomNode.commonSettlementScheme

    const schemeActor = reactor
      .use(ManageSettlementSchemeInstancesActor)
      .get(settlementSchemeId)

    if (!schemeActor) {
      logger.warn("settlement scheme actor not found", { settlementSchemeId })
      return
    }

    const peeringInfoResponse = await reactor
      .use(SendPeerMessageActor)
      .api.send.ask({
        destination: randomNode.nodeId,
        message: {
          type: "peeringInfoRequest",
          value: {
            settlementSchemeId,
          },
        },
      })

    if (!peeringInfoResponse) {
      logger.warn("peering info request failed", {
        peer: randomNode.nodeId,
        settlementSchemeId,
      })
      return
    }

    const settlementSchemeData = await schemeActor.api.createPeeringRequest.ask(
      {
        peerId: randomNode.nodeId,
        peeringInfo: peeringInfoResponse.settlementSchemeData,
      },
    )

    logger.debug(`sending peering request`, {
      to: randomNode.nodeId,
    })

    const peeringResponse = await reactor
      .use(SendPeerMessageActor)
      .api.send.ask({
        destination: randomNode.nodeId,
        message: {
          type: "peeringRequest",
          value: {
            nodeInfo: ownLinkState.lastUpdate,
            settlementSchemeId,
            settlementSchemeData: settlementSchemeData.data,
          },
        },
      })

    if (!peeringResponse?.accepted) {
      logger.debug(`peering request rejected`, {
        to: randomNode.nodeId,
      })
      return
    }

    const finalizationResult = await schemeActor.api.finalizePeeringRequest.ask(
      {
        peerId: randomNode.nodeId,
        peeringInfo: peeringInfoResponse.settlementSchemeData,
        data: peeringResponse?.data,
      },
    )

    reactor.use(NodeTableStore).updateNode(randomNode.nodeId, {
      peerState: {
        id: "peered",
        lastSeen: Date.now(),
        settlementSchemeId: randomNode.commonSettlementScheme,
        settlementSchemeState: finalizationResult.peerState,
      },
    })
  }

  return createActor((sig: DassieActorContext) => {
    sig
      .task({
        handler: addPeersIfNecessary,
        interval: PEERING_CHECK_INTERVAL,
      })
      .schedule()
  })
}
