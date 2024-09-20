import { enableMapSet, produce } from "immer"
import pMap from "p-map"

import { createActor } from "@dassie/lib-reactive"

import type {
  DassieActorContext,
  DassieReactor,
} from "../base/types/dassie-base"
import { EMPTY_UINT8ARRAY } from "../constants/general"
import { compareUint8Arrays } from "../utils/compare-typedarray"
import { NODE_LIST_HASH_POLLING_INTERVAL } from "./constants/timings"
import { SendPeerMessage } from "./functions/send-peer-message"
import { BootstrapNodeListHashesSignal } from "./signals/bootstrap-node-list-hashes"
import { BootstrapNodeListsSignal } from "./signals/bootstrap-node-lists"
import type { NodeId } from "./types/node-id"

enableMapSet()

const NODE_LIST_POLLING_CONCURRENCY = 5

export const DownloadNodeListsActor = (reactor: DassieReactor) => {
  const sendPeerMessage = reactor.use(SendPeerMessage)

  const loadNodeList = async (sourceNodeId: NodeId) => {
    const response = await sendPeerMessage({
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

  async function downloadNodeLists() {
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
  }

  return createActor(async (sig: DassieActorContext) => {
    const task = sig.task({
      handler: downloadNodeLists,
      interval: NODE_LIST_HASH_POLLING_INTERVAL,
    })

    await task.execute()
    task.schedule()
  })
}
