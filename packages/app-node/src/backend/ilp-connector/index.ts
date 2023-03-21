import { createActor } from "@dassie/lib-reactive"

import { managePrimaryIlpAddress } from "./manage-primary-ilp-address"
import { routeIlpPackets } from "./route-ilp-packets"
import { sendOutgoingPackets } from "./send-outgoing-packets"

export const startIlpConnector = () =>
  createActor((sig) => {
    sig.run(routeIlpPackets)
    sig.run(sendOutgoingPackets)
    sig.run(managePrimaryIlpAddress)
  })
