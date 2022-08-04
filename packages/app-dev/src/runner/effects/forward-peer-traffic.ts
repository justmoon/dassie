import { outgoingPeerMessageBufferTopic } from "@xen-ilp/app-node/src/peer-protocol/send-peer-messages"
import type { EffectContext } from "@xen-ilp/lib-reactive"

import { trpcClientFactory } from "../services/trpc-client"

export const forwardPeerTraffic = (sig: EffectContext) => {
  sig.onAsync(outgoingPeerMessageBufferTopic, async ({ destination }) => {
    const trpcClient = sig.reactor.useContext(trpcClientFactory)
    await trpcClient.mutation("runner.notifyPeerTraffic", {
      from: process.env["XEN_DEV_NODE_ID"] ?? "unknown",
      to: destination,
    })
  })
}
