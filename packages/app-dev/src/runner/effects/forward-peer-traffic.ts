import { outgoingPeerMessageBufferTopic } from "@dassie/app-node/src/backend/peer-protocol/send-peer-messages"
import type { EffectContext } from "@dassie/lib-reactive"

import { trpcClientService } from "../services/trpc-client"

export const forwardPeerTraffic = (sig: EffectContext) => {
  sig.onAsync(outgoingPeerMessageBufferTopic, async ({ destination }) => {
    const trpcClient = sig.get(trpcClientService)
    if (!trpcClient) return

    await trpcClient.runner.notifyPeerTraffic.mutate({
      from: process.env["DASSIE_DEV_NODE_ID"] ?? "unknown",
      to: destination,
    })
  })
}
