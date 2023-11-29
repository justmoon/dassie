import { Logger, createLogger } from "@dassie/lib-logger"

import * as namespaces from "./namespaces"

// Logger instances must be explicitly typed to avoid TypeScript error TS2775
// See: https://github.com/microsoft/TypeScript/issues/36931

export const children: Logger = createLogger(namespaces.LOGGER_CHILDREN)
export const runner: Logger = createLogger(namespaces.LOGGER_RUNNER)
export const setup: Logger = createLogger(namespaces.LOGGER_SETUP)
export const server: Logger = createLogger(namespaces.LOGGER_SERVER)
export const vite: Logger = createLogger(namespaces.LOGGER_VITE)
