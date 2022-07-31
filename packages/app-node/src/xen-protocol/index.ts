import type { EffectContext } from "@xen-ilp/lib-reactive"

import { handleXenMessages } from "./handle-xen-messages"
import { registerXenHttpHandler } from "./register-xen-http-handler"
import { sendXenMessages } from "./send-xen-messages"

export const speakXenProtocol = (sig: EffectContext) => {
  // Handle incoming Xen messages via HTTP
  sig.use(registerXenHttpHandler)
  sig.use(handleXenMessages)

  // Send outgoing Xen messages
  sig.use(sendXenMessages)
}
