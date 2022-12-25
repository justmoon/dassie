import type { EffectContext } from "@dassie/lib-reactive"

import { handleIldcpRequests } from "./handle-ildcp-requests"

export const startIldcpServer = (sig: EffectContext) => {
  sig.run(handleIldcpRequests)
}
