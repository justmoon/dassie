import { peersComputation } from "@dassie/app-node/src/backend/peer-protocol/computed/peers"
import { compareSets } from "@dassie/app-node/src/backend/utils/compare-sets"
import { createActor } from "@dassie/lib-reactive"

import { trpcClientService } from "../services/trpc-client"

export const reportPeeringState = () =>
  createActor(async (sig) => {
    const peers = sig.get(peersComputation, (peersSet) => peersSet, compareSets)

    const trpcClient = sig.get(trpcClientService)
    if (!trpcClient) return

    await trpcClient.runner.notifyPeerState.mutate({
      nodeId: process.env["DASSIE_DEV_NODE_ID"] ?? "unknown",
      peers: [...peers],
    })
  })
