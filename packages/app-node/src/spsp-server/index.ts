import type { EffectContext } from "@dassie/lib-reactive"

import { handleSpspRequests } from "./handle-spsp-requests"
import { sendSpspPayments } from "./send-spsp-payments"
import { streamServerService } from "./stream-server"

export const startSpspServer = async (sig: EffectContext) => {
  sig.run(sig.use(streamServerService).effect)
  await sig.run(handleSpspRequests)
  await sig.run(sendSpspPayments)
}
