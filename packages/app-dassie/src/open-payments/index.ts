import { createActor } from "@dassie/lib-reactive"

import type { DassieActorContext } from "../base/types/dassie-base"
import { HandleIncomingPaymentsActor } from "./handle-incoming-payments"
import { HandleWalletRequestsActor } from "./handle-wallet-requests"
import { StreamServerServiceActor } from "./stream-server"

export const OpenPaymentsServerActor = () =>
  createActor(async (sig: DassieActorContext) => {
    await sig.run(StreamServerServiceActor)
    sig.run(HandleIncomingPaymentsActor)
    sig.run(HandleWalletRequestsActor)
  })
