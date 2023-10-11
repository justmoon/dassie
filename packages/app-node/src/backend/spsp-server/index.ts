import { createActor } from "@dassie/lib-reactive"

import { HandleSpspRequestsActor } from "./handle-spsp-requests"
import { ManagePluginsActor } from "./manage-plugins"
import { SendSpspPaymentsActor } from "./send-spsp-payments"
import { StreamServerServiceActor } from "./stream-server"

export const SpspServerActor = () =>
  createActor(async (sig) => {
    sig.run(ManagePluginsActor)
    await sig.run(StreamServerServiceActor)
    sig.run(HandleSpspRequestsActor)
    await sig.run(SendSpspPaymentsActor)
  })
