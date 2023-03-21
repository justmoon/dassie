import { outgoingPeerMessageTopic } from "@dassie/app-node/src/backend/peer-protocol/actions/send-peer-message"
import { createActor } from "@dassie/lib-reactive"

import { trpcClientService } from "../services/trpc-client"

export const forwardPeerTraffic = () =>
  createActor((sig) => {
    sig.onAsync(outgoingPeerMessageTopic, async ({ destination }) => {
      const trpcClient = sig.get(trpcClientService)
      if (!trpcClient) return

      await trpcClient.runner.notifyPeerTraffic.mutate({
        from: process.env["DASSIE_DEV_NODE_ID"] ?? "unknown",
        to: destination,
      })
    })
  })
