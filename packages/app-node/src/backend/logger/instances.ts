import { createLogger } from "@dassie/lib-logger"

import * as namespaces from "./namespaces"

export const accounting = createLogger(namespaces.LOGGER_ACCOUNTING)
export const btp = createLogger(namespaces.LOGGER_BTP)
export const connector = createLogger(namespaces.LOGGER_CONNECTOR)
export const daemon = createLogger(namespaces.LOGGER_DAEMON)
export const exchange = createLogger(namespaces.LOGGER_EXCHANGE)
export const http = createLogger(namespaces.LOGGER_HTTP)
export const ildcp = createLogger(namespaces.LOGGER_ILDCP)
export const ilpHttp = createLogger(namespaces.LOGGER_ILP_HTTP)
export const ipc = createLogger(namespaces.LOGGER_IPC)
export const payment = createLogger(namespaces.LOGGER_PAYMENT)
export const peerProtocol = createLogger(namespaces.LOGGER_PEER_PROTOCOL)
export const settlement = createLogger(namespaces.LOGGER_SETTLEMENT)
export const settlementXrpl = createLogger(namespaces.LOGGER_SETTLEMENT_XRPL)
