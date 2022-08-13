import { EffectContext, createReactor } from "@dassie/lib-reactive"

import { registerRootRoute, serveHttp } from "./http-server/serve-http"
import { attachLogger } from "./logger"
import { speakPeerProtocol } from "./peer-protocol"
import { loadInitialPeers } from "./peer-protocol/load-initial-peers"
import { registerUplinkHttpUpgrade } from "./uplink/register-uplink-http-upgrade"

export const rootEffect = async (sig: EffectContext) => {
  sig.run(attachLogger)
  sig.run(serveHttp)
  sig.run(registerRootRoute)
  sig.run(registerUplinkHttpUpgrade)

  await sig.run(speakPeerProtocol)
  sig.run(loadInitialPeers)
}

export const start = () => createReactor(rootEffect)
