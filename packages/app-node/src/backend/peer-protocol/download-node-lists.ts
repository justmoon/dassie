import { enableMapSet, produce } from "immer"
import pMap from "p-map"

import { Reactor, createActor } from "@dassie/lib-reactive"

import { EMPTY_UINT8ARRAY } from "../../common/constants/general"
import { compareUint8Arrays } from "../utils/compare-typedarray"
import { SendPeerMessageActor } from "./actors/send-peer-message"
import { NODE_LIST_HASH_POLLING_INTERVAL } from "./constants/timings"
import { BootstrapNodeListHashesSignal } from "./signals/bootstrap-node-list-hashes"
import { BootstrapNodeListsSignal } from "./signals/bootstrap-node-lists"
import { NodeId } from "./types/node-id"

enableMapSet()

const NODE_LIST_POLLING_CONCURRENCY = 5

export const DownloadNodeListsActor = (reactor: Reactor) => {
  const loadNodeList = async (sourceNodeId: NodeId) => {
    const response = await reactor.use(SendPeerMessageActor).api.send.ask({
      destination: sourceNodeId,
      message: {
        type: "nodeListRequest",
        value: {},
      },
    })

    return response?.value
  }

  const bootstrapNodeListHashesSignal = reactor.use(
    BootstrapNodeListHashesSignal,
  )
  const bootstrapNodeListsSignal = reactor.use(BootstrapNodeListsSignal)

  return createActor(async (sig) => {
    try {
      const bootstrapNodeLists = bootstrapNodeListsSignal.read()
      const changedHashes = [...bootstrapNodeListHashesSignal.read().entries()]
        .map(([nodeId, hash]) => ({ nodeId, hash }))
        .filter(
          ({ nodeId, hash }) =>
            !compareUint8Arrays(
              bootstrapNodeLists.get(nodeId)?.hash ?? EMPTY_UINT8ARRAY,
              hash,
            ),
        )

      const nodeLists = await pMap(
        changedHashes.map(({ nodeId }) => nodeId),
        loadNodeList,
        {
          concurrency: NODE_LIST_POLLING_CONCURRENCY,
        },
      )

      for (const [nodeIndex, nodeList] of nodeLists.entries()) {
        if (nodeList == null) {
          continue
        }

        bootstrapNodeListsSignal.update(
          produce((draft) => {
            draft.set(changedHashes[nodeIndex]!.nodeId, {
              hash: changedHashes[nodeIndex]!.hash,
              entries: nodeList,
            })
          }),
        )
      }
    } finally {
      sig.timeout(sig.wake, NODE_LIST_HASH_POLLING_INTERVAL)
    }
  })
}
