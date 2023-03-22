import { createActor } from "@dassie/lib-reactive"

import { handleIncomingPayments } from "./handle-incoming-payments"

export const startOpenPaymentsServer = () =>
  createActor((sig) => {
    sig.run(handleIncomingPayments)
  })
