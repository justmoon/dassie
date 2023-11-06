import pMap from "p-map"

import { createActor } from "@dassie/lib-reactive"

import { NodeIdSignal } from "../ilp-connector/computed/node-id"
import { SendPeerMessageActor } from "./actors/send-peer-message"
import { BootstrapNodeListsSignal } from "./signals/bootstrap-node-lists"
import { NodeTableStore } from "./stores/node-table"

const REGISTRATION_CONCURRENCY = 5

export const RegisterOurselvesActor = () =>
  createActor(async (sig) => {
    const bootstrapNodeLists = sig.get(BootstrapNodeListsSignal)
    const ourNodeId = sig.get(NodeIdSignal)
    const ourNodeInfo = sig.get(NodeTableStore, (store) => store.get(ourNodeId))
      ?.linkState?.lastUpdate

    if (!ourNodeInfo) return

    await pMap(
      bootstrapNodeLists.entries(),
      async ([bootstrapNodeId, nodeList]) => {
        if (nodeList.entries.includes(ourNodeId)) return

        await sig.use(SendPeerMessageActor).api.send.ask({
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