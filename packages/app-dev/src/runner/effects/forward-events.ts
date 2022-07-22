import { EffectContext, debugFirehose } from "@xen-ilp/lib-reactive"

import { messageCache } from "../services/message-cache"
import { trpcClientFactory } from "../services/trpc-client"

let messageUniqueId = 0

export const forwardEvents = (sig: EffectContext) => {
  sig.onAsync(debugFirehose, async ({ topic, message }) => {
    const cache = sig.reactor.fromContext(messageCache)
    cache.set(messageUniqueId, message)
    messageUniqueId++

    const trpcClient = sig.reactor.fromContext(trpcClientFactory)
    await trpcClient.mutation("runner.notifyTopicMessage", [
      process.env["XEN_DEV_NODE_ID"] ?? "unknown",
      topic.name,
      messageUniqueId,
    ])
  })
}
