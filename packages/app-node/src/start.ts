import { EffectContext, createReactor } from "@xen-ilp/lib-reactive"

import { registerRootRoute, serveHttp } from "./http-server/serve-http"
import { forwardLinkStateUpdate } from "./peering/forward-link-state-update"
import { greetPeers } from "./peering/greet-peers"
import { loadInitialPeers } from "./peering/load-initial-peers"
import { publishLinkStateUpdate } from "./peering/publish-link-state-update"
import { registerUplinkHttpUpgrade } from "./uplink/register-uplink-http-upgrade"
import { speakXenProtocol } from "./xen-protocol"

export const rootEffect = (sig: EffectContext) => {
  sig.use(speakXenProtocol)
  sig.use(greetPeers)
  sig.use(publishLinkStateUpdate)
  sig.use(forwardLinkStateUpdate)
  sig.use(serveHttp)
  sig.use(registerRootRoute)
  sig.use(registerUplinkHttpUpgrade)

  sig.use(loadInitialPeers)
}

export const start = () => createReactor(rootEffect)
