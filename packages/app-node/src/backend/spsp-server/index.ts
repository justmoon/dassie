import { createActor } from "@dassie/lib-reactive"

import { handleSpspRequests } from "./handle-spsp-requests"
import { managePlugins } from "./manage-plugins"
import { sendSpspPayments } from "./send-spsp-payments"
import { streamServerService } from "./stream-server"

export const startSpspServer = () =>
  createActor(async (sig) => {
    sig.run(managePlugins, undefined, { register: true })
    await sig.run(streamServerService, undefined, { register: true }).result
    sig.run(handleSpspRequests)
    await sig.run(sendSpspPayments).result
  })
