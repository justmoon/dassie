import { createActor, createReactor } from "@dassie/lib-reactive"

import { startAccounting } from "./accounting"
import { startBtpServer } from "./btp-server"
import { signerService } from "./crypto/signer"
import { startExchangeRates } from "./exchange-rates"
import { startHttpServer } from "./http-server"
import { startIldcpServer } from "./ildcp-server"
import { startIlpConnector } from "./ilp-connector"
import { attachLogger } from "./logger"
import { startOpenPaymentsServer } from "./open-payments"
import { speakPeerProtocol } from "./peer-protocol"
import { doRouting } from "./routing"
import { startSpspServer } from "./spsp-server"
import { startStatisticsServer } from "./statistics"
import { startSubnets } from "./subnets"
import { startTrpcServer } from "./trpc-server"

export const rootActor = () =>
  createActor(async (sig) => {
    sig.run(signerService)
    sig.run(attachLogger)
    sig.run(startAccounting)
    sig.run(startIlpConnector)
    sig.run(startHttpServer)
    sig.run(startBtpServer)
    sig.run(startTrpcServer)
    sig.run(startIldcpServer)
    await sig.run(startExchangeRates)
    sig.run(startSubnets)
    await sig.run(startSpspServer)
    sig.run(startOpenPaymentsServer)
    sig.run(startStatisticsServer)

    await sig.run(speakPeerProtocol)
    sig.run(doRouting)
  })

export const start = () => createReactor(rootActor)
