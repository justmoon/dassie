import { createActor } from "@dassie/lib-reactive"

import { HandleIncomingPaymentsActor } from "./handle-incoming-payments"

export const OpenPaymentsServerActor = () =>
  createActor((sig) => {
    sig.run(HandleIncomingPaymentsActor)
  })
