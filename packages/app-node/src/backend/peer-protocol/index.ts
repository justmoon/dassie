import type { EffectContext } from "@dassie/lib-reactive"

import { forwardLinkStateUpdate } from "./forward-link-state-update"
import { registerPeerHttpHandler } from "./register-peer-http-handler"
import { sendHeartbeats } from "./send-heartbeats"
import { sendPeerMessages } from "./send-peer-messages"

export const speakPeerProtocol = (sig: EffectContext) => {
  // Handle incoming Dassie messages via HTTP
  sig.run(registerPeerHttpHandler)

  // Send outgoing Dassie messages
  sig.run(sendPeerMessages)

  sig.run(sendHeartbeats)
  sig.run(forwardLinkStateUpdate)
}
