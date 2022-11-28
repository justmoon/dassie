import type { EffectContext } from "@dassie/lib-reactive"

import { managePrimaryIlpAddress } from "./manage-primary-ilp-address"
import { routeIlpPackets } from "./route-ilp-packets"
import { sendOutgoingPackets } from "./send-outgoing-packets"

export const startIlpConnector = (sig: EffectContext) => {
  sig.run(routeIlpPackets)
  sig.run(sendOutgoingPackets)
  sig.run(managePrimaryIlpAddress)
}
