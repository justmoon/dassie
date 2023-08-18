import { createActor, createReactor } from "@dassie/lib-reactive"

import { startAccounting } from "./accounting"
import { startAcmeCertificateManager } from "./acme-certificate-manager"
import { startAuthenticationFeature } from "./authentication"
import { startBtpServer } from "./btp-server"
import { hasNodeIdentityComputed } from "./config/computed/has-node-identity"
import { hasTlsComputed } from "./config/computed/has-tls"
import { signerService } from "./crypto/signer"
import { startExchangeRates } from "./exchange-rates"
import { startHttpServer } from "./http-server"
import { startIldcpServer } from "./ildcp-server"
import { startIlpConnector } from "./ilp-connector"
import { startLocalRpcServer } from "./local-ipc-server"
import { startLogger } from "./logger"
import { daemon as logger } from "./logger/instances"
import { startOpenPaymentsServer } from "./open-payments"
import { speakPeerProtocol } from "./peer-protocol"
import { doRouting } from "./routing"
import { startSettlementSchemes } from "./settlement-schemes"
import { startSpspServer } from "./spsp-server"
import { startStatisticsServer } from "./statistics"
import { supportSystemd } from "./systemd"
import { startTrpcServer } from "./trpc-server"

export const startTlsDependentServices = () =>
  createActor((sig) => {
    const hasTls = sig.get(hasTlsComputed)

    if (!hasTls) {
      logger.warn("Web UI is not configured, run `dassie init`")
      return
    }

    sig.run(startTrpcServer)
  })

export const startNodeIdentityDependentServices = () =>
  createActor(async (sig) => {
    const hasNodeIdentity = sig.get(hasNodeIdentityComputed)

    if (!hasNodeIdentity) {
      logger.warn("Node identity is not configured")
      return
    }

    sig.run(signerService)
    sig.run(startAccounting)
    sig.run(startIlpConnector)
    sig.run(startBtpServer)
    sig.run(startIldcpServer)
    await sig.run(startExchangeRates)

    sig.run(startAuthenticationFeature)
    sig.run(startSettlementSchemes)
    await sig.run(startSpspServer)
    sig.run(startOpenPaymentsServer)
    sig.run(startStatisticsServer)

    await sig.run(speakPeerProtocol)
    sig.run(doRouting)
  })

export const daemonActor = () =>
  createActor(async (sig) => {
    sig.run(startLogger)
    sig.run(startLocalRpcServer)
    sig.run(supportSystemd)
    sig.run(startHttpServer)
    sig.run(startAcmeCertificateManager)

    sig.run(startTlsDependentServices)
    await sig.run(startNodeIdentityDependentServices)
  })

export const startDaemon = () => createReactor(daemonActor)
