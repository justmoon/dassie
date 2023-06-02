import { createActor } from "@dassie/lib-reactive"

import { handleSpspRequests } from "./handle-spsp-requests"
import { managePlugins } from "./manage-plugins"
import { sendSpspPayments } from "./send-spsp-payments"
import { streamServerService } from "./stream-server"

export const startSpspServer = () =>
  createActor(async (sig) => {
    sig.run(managePlugins)
    await sig.run(streamServerService)
    sig.run(handleSpspRequests)
    await sig.run(sendSpspPayments)
  })
