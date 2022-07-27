import { EffectContext, createReactor } from "@xen-ilp/lib-reactive"

import { httpServerValue } from "./http-server/http-server"
import { websocketServerValue } from "./http-server/websocket-server"
import { initialPeerLoader } from "./peering/initial-peer-loader"
import { peerGreeter } from "./peering/peer-greeter"
import { xenProtocol } from "./xen-protocol"

export const rootEffect = (sig: EffectContext) => {
  sig.use(xenProtocol)
  sig.use(peerGreeter)
  sig.read(httpServerValue)
  sig.read(websocketServerValue)

  sig.use(initialPeerLoader)
}

export const start = () => createReactor(rootEffect)
