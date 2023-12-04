import { createActor, createReactor } from "@dassie/lib-reactive"
import { createNodeRuntime } from "@dassie/lib-reactive-io/node"

import { AccountingActor } from "./accounting"
import { AcmeCertificateManagerActor } from "./acme-certificate-manager"
import { AuthenticationFeatureActor } from "./authentication"
import { SetupUrlSignal } from "./authentication/computed/setup-url"
import { DassieActorContext } from "./base/types/dassie-base"
import { BtpServerActor } from "./btp-server"
import { HasNodeIdentitySignal } from "./config/computed/has-node-identity"
import { HasTlsSignal } from "./config/computed/has-tls"
import { ExchangeRatesActor } from "./exchange"
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
    const hasTls = sig.readAndTrack(HasTlsSignal)

    if (!hasTls) {
      logger.warn("Web UI is not configured, run `dassie init`")
      return
    }

    sig.run(TrpcServerActor)
    sig.run(AuthenticationFeatureActor)
  })

export const StartNodeIdentityDependentServicesActor = () =>
  createActor(async (sig: DassieActorContext) => {
    const hasNodeIdentity = sig.readAndTrack(HasNodeIdentitySignal)

    if (!hasNodeIdentity) {
      const hasTls = sig.read(HasTlsSignal)
      if (hasTls) {
        const setupUrl = sig.readAndTrack(SetupUrlSignal)
        logger.warn(`Node identity is not configured, visit ${setupUrl}`)
      }
      return
    }

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
  createActor(async (sig: DassieActorContext) => {
    sig.run(LoggerActor)
    sig.run(LocalRpcServerActor)
    sig.run(SystemdActor)
    sig.run(HttpServerActor)
    sig.run(AcmeCertificateManagerActor)

    sig.run(StartTlsDependentServicesActor)
    await sig.run(StartNodeIdentityDependentServicesActor)
  })

export const startDaemon = () => createReactor(DaemonActor, createNodeRuntime())
