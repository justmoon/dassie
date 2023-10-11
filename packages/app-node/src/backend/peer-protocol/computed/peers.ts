import { enableMapSet, produce } from "immer"

import { Reactor, createComputed } from "@dassie/lib-reactive"
import { UnreachableCaseError } from "@dassie/lib-type-utils"

import { NodeTableStore } from "../stores/node-table"
import { NodeId } from "../types/node-id"

enableMapSet()

export const PeersSignal = (reactor: Reactor) =>
  createComputed(reactor.lifecycle, () => {
    const nodeTable = reactor.use(NodeTableStore)

    const initialSet = new Set<NodeId>()

    for (const node of nodeTable.read().values()) {
      if (node.peerState.id === "peered") {
        initialSet.add(node.nodeId)
      }
    }

    nodeTable.changes.on(reactor.lifecycle, ([actionId, parameters]) => {
      const peersSignal = reactor.use(PeersSignal)
      const peersSet = peersSignal.read()
      const priorSize = peersSet.size

      const newSet = produce(peersSet, (draft) => {
        switch (actionId) {
          case "addNode": {
            const { peerState, nodeId } = parameters[0]
            if (peerState.id === "peered") {
              draft.add(nodeId)
            }
            break
          }
          case "updateNode": {
            const nodeKey = parameters[0]
            const { peerState } = parameters[1]
            if (peerState?.id === "peered") {
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
        peersSignal.write(newSet)
      }
    })

    return initialSet
  })

export const PeersArraySignal = (reactor: Reactor) =>
  createComputed(reactor.lifecycle, (sig) => [
    ...sig.get(reactor.use(PeersSignal)),
  ])
