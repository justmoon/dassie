import { createLogger } from "@dassie/lib-logger"
import { createActor, createReactor } from "@dassie/lib-reactive"

import { startAccounting } from "./accounting"
import { startBtpServer } from "./btp-server"
import { databaseConfigPlain } from "./config/database-config"
import { signerService } from "./crypto/signer"
import { startExchangeRates } from "./exchange-rates"
import { startHttpServer } from "./http-server"
import { startIldcpServer } from "./ildcp-server"
import { startIlpConnector } from "./ilp-connector"
import { startLocalRpcServer } from "./local-ipc-server"
import { attachLogger } from "./logger"
import { startOpenPaymentsServer } from "./open-payments"
import { speakPeerProtocol } from "./peer-protocol"
import { doRouting } from "./routing"
import { startSpspServer } from "./spsp-server"
import { startStatisticsServer } from "./statistics"
import { startSubnets } from "./subnets"
import { supportSystemd } from "./systemd"
import { startTrpcServer } from "./trpc-server"

const logger = createLogger("das:start")

export const daemonActor = () =>
  createActor(async (sig) => {
    const config = sig.use(databaseConfigPlain)
    sig.run(attachLogger)
    sig.run(startLocalRpcServer)
    sig.run(supportSystemd)

    if (!config.hasWebUi) {
      logger.warn("Web UI is not configured, run `dassie init`")
      return
    }

    sig.run(startHttpServer)
    sig.run(startTrpcServer)

    if (!config.hasNodeIdentity) {
      logger.warn("Node identity is not configured")
      return
    }

    sig.run(signerService)
    sig.run(startAccounting)
    sig.run(startIlpConnector)
    sig.run(startBtpServer)
    sig.run(startIldcpServer)
    await sig.run(startExchangeRates)

    sig.run(startSubnets)
    await sig.run(startSpspServer)
    sig.run(startOpenPaymentsServer)
    sig.run(startStatisticsServer)

    await sig.run(speakPeerProtocol)
    sig.run(doRouting)
  })

export const startDaemon = () => createReactor(daemonActor)
