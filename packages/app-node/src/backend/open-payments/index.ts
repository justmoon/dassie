import type { EffectContext } from "@dassie/lib-reactive"

import { handleIncomingPayments } from "./handle-incoming-payments"

export const startOpenPaymentsServer = async (sig: EffectContext) => {
  await sig.run(handleIncomingPayments)
}
