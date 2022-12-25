import type { EffectContext } from "@dassie/lib-reactive"

import { forwardLinkStateUpdate } from "./forward-link-state-update"
import { greetPeers } from "./greet-peers"
import { registerPeerHttpHandler } from "./register-peer-http-handler"
import { sendPeerMessages } from "./send-peer-messages"

export const speakPeerProtocol = async (sig: EffectContext) => {
  // Handle incoming Dassie messages via HTTP
  sig.run(registerPeerHttpHandler)

  // Send outgoing Dassie messages
  sig.run(sendPeerMessages)

  await sig.run(greetPeers)
  sig.run(forwardLinkStateUpdate)
}
