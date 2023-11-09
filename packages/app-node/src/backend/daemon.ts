import { createActor, createReactor } from "@dassie/lib-reactive"

import { AccountingActor } from "./accounting"
import { AcmeCertificateManagerActor } from "./acme-certificate-manager"
import { AuthenticationFeatureActor } from "./authentication"
import { SetupUrlSignal } from "./authentication/computed/setup-url"
import { BtpServerActor } from "./btp-server"
import { HasNodeIdentitySignal } from "./config/computed/has-node-identity"
import { HasTlsSignal } from "./config/computed/has-tls"
import { SignerActor } from "./crypto/signer"
import { ExchangeRatesActor } from "./exchange-rates"
import { HttpServerActor } from "./http-server"
import { IldcpServerActor } from "./ildcp-server"
import { IlpConnectorActor } from "./ilp-connector"
import { IlpHttpActor } from "./ilp-http"
import { LocalRpcServerActor } from "./local-ipc-server"
import { LoggerActor } from "./logger"
import { daemon as logger } from "./logger/instances"
import { OpenPaymentsServerActor } from "./open-payments"
import { PeerProtocolActor } from "./peer-protocol"
import { RoutingActor } from "./routing"
import { SettlementSchemesActor } from "./settlement-schemes"
import { SpspServerActor } from "./spsp-server"
import { StatisticsServerActor } from "./statistics"
import { SystemdActor } from "./systemd"
import { TrpcServerActor } from "./trpc-server"

export const StartTlsDependentServicesActor = () =>
  createActor((sig) => {
    const hasTls = sig.get(HasTlsSignal)

    if (!hasTls) {
      logger.warn("Web UI is not configured, run `dassie init`")
      return
    }

    sig.run(TrpcServerActor)
    sig.run(AuthenticationFeatureActor)
  })

export const StartNodeIdentityDependentServicesActor = () =>
  createActor(async (sig) => {
    const hasNodeIdentity = sig.get(HasNodeIdentitySignal)

    if (!hasNodeIdentity) {
      const hasTls = sig.use(HasTlsSignal).read()
      if (hasTls) {
        const setupUrl = sig.get(SetupUrlSignal)
        logger.warn(`Node identity is not configured, visit ${setupUrl}`)
      }
      return
    }

    sig.run(SignerActor)
    sig.run(AccountingActor)
    sig.run(IlpConnectorActor)
    sig.run(BtpServerActor)
    sig.run(IldcpServerActor)
    sig.run(IlpHttpActor)
    await sig.run(ExchangeRatesActor)

    sig.run(SettlementSchemesActor)
    await sig.run(SpspServerActor)
    sig.run(OpenPaymentsServerActor)
    sig.run(StatisticsServerActor)

    await sig.run(PeerProtocolActor)
    sig.run(RoutingActor)
  })

export const DaemonActor = () =>
  createActor(async (sig) => {
    sig.run(LoggerActor)
    sig.run(LocalRpcServerActor)
    sig.run(SystemdActor)
    sig.run(HttpServerActor)
    sig.run(AcmeCertificateManagerActor)

    sig.run(StartTlsDependentServicesActor)
    await sig.run(StartNodeIdentityDependentServicesActor)
  })

export const startDaemon = () => createReactor(DaemonActor)
