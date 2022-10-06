import { outgoingPeerMessageBufferTopic } from "@dassie/app-node/src/peer-protocol/send-peer-messages"
import type { EffectContext } from "@dassie/lib-reactive"

import { trpcClientService } from "../services/trpc-client"

export const forwardPeerTraffic = (sig: EffectContext) => {
  sig.onAsync(outgoingPeerMessageBufferTopic, async ({ destination }) => {
    const trpcClient = sig.get(trpcClientService)
    if (!trpcClient) return

    await trpcClient.mutation("runner.notifyPeerTraffic", {
      from: process.env["DASSIE_DEV_NODE_ID"] ?? "unknown",
      to: destination,
    })
  })
}
