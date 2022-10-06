import { EffectContext, createReactor } from "@dassie/lib-reactive"

import { pingBeacons } from "./beacon/ping-beacons"
import { registerBtpHttpUpgrade } from "./btp-server/register-btp-http-upgrade"
import { signerService } from "./crypto/signer"
import { openDatabase } from "./database/open-database"
import { serveHttp } from "./http-server/serve-http"
import { routeIlpPackets } from "./ilp-connector/route-ilp-packets"
import { sendOutgoingPackets } from "./ilp-connector/send-outgoing-packets"
import { attachLogger } from "./logger"
import { speakPeerProtocol } from "./peer-protocol"
import { loadInitialPeers } from "./peer-protocol/load-initial-peers"
import { startSpspServer } from "./spsp-server"
import { registerTrpcHttpUpgrade } from "./trpc-server/trpc-server"

export const rootEffect = async (sig: EffectContext) => {
  sig.run(sig.use(signerService).effect)
  sig.run(openDatabase)
  sig.run(attachLogger)
  sig.run(serveHttp)
  sig.run(registerBtpHttpUpgrade)
  sig.run(registerTrpcHttpUpgrade)
  sig.run(routeIlpPackets)
  sig.run(sendOutgoingPackets)
  await sig.run(startSpspServer)

  await sig.run(speakPeerProtocol)
  sig.run(loadInitialPeers)

  sig.run(pingBeacons)
}

export const start = () => createReactor(rootEffect)
