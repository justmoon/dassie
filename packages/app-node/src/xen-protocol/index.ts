import type { EffectContext } from "@xen-ilp/lib-reactive"

import { incomingXenMessageHandler } from "./incoming-xen-message-handler"
import { incomingXenMessageParser } from "./incoming-xen-message-parser"
import { outgoingXenMessageSender } from "./outgoing-xen-message-sender"
import { outgoingXenMessageSigner } from "./outgoing-xen-message-signer"

export const xenProtocol = (sig: EffectContext) => {
  // Incoming Xen message pipeline
  sig.use(incomingXenMessageParser)
  sig.use(incomingXenMessageHandler)

  // Outgoing Xen message pipeline
  sig.use(outgoingXenMessageSigner)
  sig.use(outgoingXenMessageSender)
}
