import { createActor } from "@dassie/lib-reactive"

import { handleIncomingPayments } from "./handle-incoming-payments"

export const startOpenPaymentsServer = () =>
  createActor(async (sig) => {
    await sig.run(handleIncomingPayments)
  })
