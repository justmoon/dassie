import { EffectContext, createReactor } from "@dassie/lib-reactive"

import { pingBeacons } from "./beacon/ping-beacons"
import { registerBtpHttpUpgrade } from "./btp-server/register-btp-http-upgrade"
import { registerBtpIlpSender } from "./btp-server/register-btp-ilp-sender"
import { signerService } from "./crypto/signer"
import { registerRootRoute, serveHttp } from "./http-server/serve-http"
import { routeIlpPackets } from "./ilp-connector/route-ilp-packets"
import { attachLogger } from "./logger"
import { speakPeerProtocol } from "./peer-protocol"
import { loadInitialPeers } from "./peer-protocol/load-initial-peers"
import { registerUplinkHttpUpgrade } from "./uplink/register-uplink-http-upgrade"

export const rootEffect = async (sig: EffectContext) => {
  sig.run(sig.use(signerService).effect)
  sig.run(attachLogger)
  sig.run(serveHttp)
  sig.run(registerRootRoute)
  sig.run(registerUplinkHttpUpgrade)
  sig.run(registerBtpHttpUpgrade)
  sig.run(registerBtpIlpSender)
  sig.run(routeIlpPackets)

  await sig.run(speakPeerProtocol)
  sig.run(loadInitialPeers)

  sig.run(pingBeacons)
}

export const start = () => createReactor(rootEffect)
