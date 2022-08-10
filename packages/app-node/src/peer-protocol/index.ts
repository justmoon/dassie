import type { EffectContext } from "@dassie/lib-reactive"

import { calculateRoutes } from "./calculate-routes"
import { forwardLinkStateUpdate } from "./forward-link-state-update"
import { greetPeers } from "./greet-peers"
import { handlePeerMessages } from "./handle-peer-messages"
import { maintainOwnNodeTableEntry } from "./maintain-own-node-table-entry"
import { registerPeerHttpHandler } from "./register-peer-http-handler"
import { sendPeerMessages } from "./send-peer-messages"

export const speakPeerProtocol = async (sig: EffectContext) => {
  // Handle incoming Dassie messages via HTTP
  sig.use(registerPeerHttpHandler)
  sig.use(handlePeerMessages)

  // Send outgoing Dassie messages
  sig.use(sendPeerMessages)

  await sig.use(greetPeers)
  // sig.use(publishLinkStateUpdate)
  await sig.use(maintainOwnNodeTableEntry)
  sig.use(forwardLinkStateUpdate)
  sig.use(calculateRoutes)
}
