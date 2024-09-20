import { type Logger, createLogger } from "@dassie/lib-logger"

import * as namespaces from "./namespaces"

// Logger instances must be explicitly typed to avoid TypeScript error TS2775
// See: https://github.com/microsoft/TypeScript/issues/36931

export const accounting: Logger = createLogger(namespaces.LOGGER_ACCOUNTING)
export const acme: Logger = createLogger(namespaces.LOGGER_ACME)
export const btp: Logger = createLogger(namespaces.LOGGER_BTP)
export const connector: Logger = createLogger(namespaces.LOGGER_CONNECTOR)
export const crypto: Logger = createLogger(namespaces.LOGGER_CRYPTO)
export const daemon: Logger = createLogger(namespaces.LOGGER_DAEMON)
export const database: Logger = createLogger(namespaces.LOGGER_DATABASE)
export const exchange: Logger = createLogger(namespaces.LOGGER_EXCHANGE)
export const http: Logger = createLogger(namespaces.LOGGER_HTTP)
export const ildcp: Logger = createLogger(namespaces.LOGGER_ILDCP)
export const ilpHttp: Logger = createLogger(namespaces.LOGGER_ILP_HTTP)
export const ipc: Logger = createLogger(namespaces.LOGGER_IPC)
export const payment: Logger = createLogger(namespaces.LOGGER_PAYMENT)
export const peerProtocol: Logger = createLogger(
  namespaces.LOGGER_PEER_PROTOCOL,
)
export const settlement: Logger = createLogger(namespaces.LOGGER_SETTLEMENT)
export const settlementStub: Logger = createLogger(
  namespaces.LOGGER_SETTLEMENT_STUB,
)
export const settlementXrpl: Logger = createLogger(
  namespaces.LOGGER_SETTLEMENT_XRPL,
)
export const systemd: Logger = createLogger(namespaces.LOGGER_SYSTEMD)
