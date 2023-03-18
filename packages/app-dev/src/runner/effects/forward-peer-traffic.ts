import { outgoingPeerMessageTopic } from "@dassie/app-node/src/backend/peer-protocol/actions/send-peer-message"
import type { EffectContext } from "@dassie/lib-reactive"

import { trpcClientService } from "../services/trpc-client"

export const forwardPeerTraffic = (sig: EffectContext) => {
  sig.onAsync(outgoingPeerMessageTopic, async ({ destination }) => {
    const trpcClient = sig.get(trpcClientService)
    if (!trpcClient) return

    await trpcClient.runner.notifyPeerTraffic.mutate({
      from: process.env["DASSIE_DEV_NODE_ID"] ?? "unknown",
      to: destination,
    })
  })
}
