import type { EffectContext } from "@xen-ilp/lib-reactive"

import { forwardLinkStateUpdate } from "./forward-link-state-update"
import { greetPeers } from "./greet-peers"
import { handlePeerMessages } from "./handle-peer-messages"
import { publishLinkStateUpdate } from "./publish-link-state-update"
import { registerPeerHttpHandler } from "./register-peer-http-handler"
import { sendPeerMessages } from "./send-peer-messages"

export const speakPeerProtocol = (sig: EffectContext) => {
  // Handle incoming Xen messages via HTTP
  sig.use(registerPeerHttpHandler)
  sig.use(handlePeerMessages)

  // Send outgoing Xen messages
  sig.use(sendPeerMessages)

  sig.use(greetPeers)
  sig.use(publishLinkStateUpdate)
  sig.use(forwardLinkStateUpdate)
}
