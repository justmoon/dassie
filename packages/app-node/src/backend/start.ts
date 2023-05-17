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
import { startSpspServer } from "./spsp-server"
import { startStatisticsServer } from "./statistics"
import { startSubnets } from "./subnets"
import { startTrpcServer } from "./trpc-server"

export const rootActor = () =>
  createActor(async (sig) => {
    sig.run(signerService, undefined, { register: true })
    sig.run(attachLogger)
    sig.run(startAccounting)
    sig.run(startIlpConnector)
    sig.run(startHttpServer)
    sig.run(startBtpServer)
    sig.run(startTrpcServer)
    sig.run(startIldcpServer)
    await sig.run(startExchangeRates).result
    await sig.run(startSubnets).result
    await sig.run(startSpspServer).result
    sig.run(startOpenPaymentsServer)
    sig.run(startStatisticsServer)

    sig.run(speakPeerProtocol)
  })

export const start = () => createReactor(rootActor)
