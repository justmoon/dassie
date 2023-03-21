import { peerTableStore } from "@dassie/app-node/src/backend/peer-protocol/stores/peer-table"
import { compareArrays } from "@dassie/app-node/src/backend/utils/compare-sets"
import { createActor } from "@dassie/lib-reactive"

import { trpcClientService } from "../services/trpc-client"

export const reportPeeringState = () =>
  createActor(async (sig) => {
    const peers = sig.get(
      peerTableStore,
      (peerTable) =>
        [...peerTable]
          .filter(([, entry]) => entry.state.id === "peered")
          .map(([key]) => key),
      compareArrays
    )

    const trpcClient = sig.get(trpcClientService)
    if (!trpcClient) return

    await trpcClient.runner.notifyPeerState.mutate({
      nodeId: process.env["DASSIE_DEV_NODE_ID"] ?? "unknown",
      peers,
    })
  })
