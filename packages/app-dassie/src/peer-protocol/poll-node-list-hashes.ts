import { enableMapSet, produce } from "immer"
import pMap from "p-map"

import { createActor } from "@dassie/lib-reactive"

import type {
  DassieActorContext,
  DassieReactor,
} from "../base/types/dassie-base"
import { EnvironmentConfig } from "../config/environment-config"
import { NODE_LIST_HASH_POLLING_INTERVAL } from "./constants/timings"
import { SendPeerMessage } from "./functions/send-peer-message"
import { BootstrapNodeListHashesSignal } from "./signals/bootstrap-node-list-hashes"
import type { NodeId } from "./types/node-id"

enableMapSet()

const NODE_LIST_POLLING_CONCURRENCY = 5

export const PollNodeListHashesActor = (reactor: DassieReactor) => {
  const sendPeerMessage = reactor.use(SendPeerMessage)
  const { bootstrapNodes } = reactor.use(EnvironmentConfig)

  const loadNodeListHash = async (sourceNodeId: NodeId) => {
    const response = await sendPeerMessage({
      destination: sourceNodeId,
      message: {
        type: "nodeListHashRequest",
        value: {},
      },
    })

    return response?.hash
  }

  const bootstrapNodeListHashesSignal = reactor.use(
    BootstrapNodeListHashesSignal,
  )

  async function queryNodeListHashes() {
    const nodeListHashes = await pMap(
      bootstrapNodes.map(({ id }) => id),
      loadNodeListHash,
      {
        concurrency: NODE_LIST_POLLING_CONCURRENCY,
      },
    )

    for (const [nodeIndex, nodeListHash] of nodeListHashes.entries()) {
      if (nodeListHash == null) {
        continue
      }

      bootstrapNodeListHashesSignal.update(
        produce((draft) => {
          draft.set(bootstrapNodes[nodeIndex]!.id, nodeListHash)
        }),
      )
    }
  }

  return createActor(async (sig: DassieActorContext) => {
    const task = sig.task({
      handler: queryNodeListHashes,
      interval: NODE_LIST_HASH_POLLING_INTERVAL,
    })
    await task.execute()
    task.schedule()
  })
}
