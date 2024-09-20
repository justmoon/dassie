import pMap from "p-map"

import { createActor } from "@dassie/lib-reactive"

import type { DassieReactor } from "../base/types/dassie-base"
import { NodeIdSignal } from "../ilp-connector/computed/node-id"
import { SendPeerMessage } from "./functions/send-peer-message"
import { BootstrapNodeListsSignal } from "./signals/bootstrap-node-lists"
import { NodeTableStore } from "./stores/node-table"

const REGISTRATION_CONCURRENCY = 5

export const RegisterOurselvesActor = (reactor: DassieReactor) => {
  const sendPeerMessage = reactor.use(SendPeerMessage)

  return createActor(async (sig) => {
    const bootstrapNodeLists = sig.readAndTrack(BootstrapNodeListsSignal)
    const ourNodeId = sig.readAndTrack(NodeIdSignal)
    const ourNodeInfo = sig.readAndTrack(NodeTableStore, (store) =>
      store.get(ourNodeId),
    )?.linkState?.lastUpdate

    if (!ourNodeInfo) return

    await pMap(
      bootstrapNodeLists.entries(),
      async ([bootstrapNodeId, nodeList]) => {
        if (nodeList.entries.includes(ourNodeId)) return

        await sendPeerMessage({
          destination: bootstrapNodeId,
          message: {
            type: "registration",
            value: {
              nodeInfo: {
                bytes: ourNodeInfo,
              },
            },
          },
        })
      },
      { concurrency: REGISTRATION_CONCURRENCY },
    )
  })
}
