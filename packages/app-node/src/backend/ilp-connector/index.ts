import type { EffectContext } from "@dassie/lib-reactive"

import { routeIlpPackets } from "./route-ilp-packets"
import { sendOutgoingPackets } from "./send-outgoing-packets"

export const startIlpConnector = (sig: EffectContext) => {
  sig.run(routeIlpPackets)
  sig.run(sendOutgoingPackets)
}
