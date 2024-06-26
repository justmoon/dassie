import { createActor } from "@dassie/lib-reactive"

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
import { IlpHttpActor } from "./ilp-http"
import { LocalRpcServerActor } from "./local-ipc-server"
import { daemon as logger } from "./logger/instances"
import { OpenPaymentsServerActor } from "./open-payments"
import { PeerProtocolActor } from "./peer-protocol"
import { PublicApiServerActor } from "./public-api-server"
import { RoutingActor } from "./routing"
import { TrpcServerActor } from "./rpc-server"
import { SettlementSchemesActor } from "./settlement-schemes"
import { SpspServerActor } from "./spsp-server"

export const StartTlsDependentServicesActor = () =>
  createActor((sig: DassieActorContext) => {
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
    sig.run(BtpServerActor)
    sig.run(IldcpServerActor)
    sig.run(IlpHttpActor)
    await sig.run(ExchangeRatesActor)

    await sig.run(SettlementSchemesActor)
    await sig.run(SpspServerActor)
    sig.run(OpenPaymentsServerActor)
    sig.run(PublicApiServerActor)

    await sig.run(PeerProtocolActor)
    sig.run(RoutingActor)
  })

export const DaemonActor = () =>
  createActor(async (sig: DassieActorContext) => {
    sig.run(LocalRpcServerActor)
    sig.run(HttpServerActor)
    await sig.run(AcmeCertificateManagerActor)

    sig.run(StartTlsDependentServicesActor)
    await sig.run(StartNodeIdentityDependentServicesActor)
  })
