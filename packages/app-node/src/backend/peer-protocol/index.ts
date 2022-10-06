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
  sig.run(registerPeerHttpHandler)
  sig.run(handlePeerMessages)

  // Send outgoing Dassie messages
  sig.run(sendPeerMessages)

  await sig.run(greetPeers)
  // sig.run(publishLinkStateUpdate)
  await sig.run(maintainOwnNodeTableEntry)
  sig.run(forwardLinkStateUpdate)
  sig.run(calculateRoutes)
}
