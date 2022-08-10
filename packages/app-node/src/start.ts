import { EffectContext, createReactor } from "@dassie/lib-reactive"

import { registerRootRoute, serveHttp } from "./http-server/serve-http"
import { attachLogger } from "./logger"
import { speakPeerProtocol } from "./peer-protocol"
import { loadInitialPeers } from "./peer-protocol/load-initial-peers"
import { registerUplinkHttpUpgrade } from "./uplink/register-uplink-http-upgrade"

export const rootEffect = (sig: EffectContext) => {
  sig.use(attachLogger)
  sig.use(speakPeerProtocol)
  sig.use(serveHttp)
  sig.use(registerRootRoute)
  sig.use(registerUplinkHttpUpgrade)

  sig.use(loadInitialPeers)
}

export const start = () => createReactor(rootEffect)
