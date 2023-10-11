import { enableMapSet, produce } from "immer"

import { Reactor, createComputed } from "@dassie/lib-reactive"
import { UnreachableCaseError } from "@dassie/lib-type-utils"

import { NodeTableStore } from "../stores/node-table"
import { NodeId } from "../types/node-id"

enableMapSet()

export const RequestedPeersSignal = (reactor: Reactor) =>
  createComputed(reactor.lifecycle, () => {
    const nodeTable = reactor.use(NodeTableStore)

    const initialSet = new Set<NodeId>()

    for (const node of nodeTable.read().values()) {
      if (node.peerState.id === "request-peering") {
        initialSet.add(node.nodeId)
      }
    }

    nodeTable.changes.on(reactor.lifecycle, ([actionId, parameters]) => {
      const requestedPeersSignal = reactor.use(RequestedPeersSignal)
      const peersSet = requestedPeersSignal.read()
      const priorSize = peersSet.size

      const newSet = produce(peersSet, (draft) => {
        switch (actionId) {
          case "addNode": {
            const { peerState, nodeId } = parameters[0]
            if (peerState.id === "request-peering") {
              draft.add(nodeId)
            }
            break
          }
          case "updateNode": {
            const nodeKey = parameters[0]
            const { peerState } = parameters[1]
            if (peerState?.id === "request-peering") {
              draft.add(nodeKey)
            } else if (peerState) {
              draft.delete(nodeKey)
            }
            break
          }
          default: {
            throw new UnreachableCaseError(actionId)
          }
        }
      })

      if (newSet.size !== priorSize) {
        requestedPeersSignal.write(newSet)
      }
    })

    return initialSet
  })

export const RequestedPeersArraySignal = (reactor: Reactor) =>
  createComputed(reactor.lifecycle, (sig) => [
    ...sig.get(reactor.use(RequestedPeersSignal)),
  ])
