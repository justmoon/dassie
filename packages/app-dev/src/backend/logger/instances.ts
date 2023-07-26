import { createLogger } from "@dassie/lib-logger"

import * as namespaces from "./namespaces"

export const children = createLogger(namespaces.LOGGER_CHILDREN)
export const runner = createLogger(namespaces.LOGGER_RUNNER)
export const setup = createLogger(namespaces.LOGGER_SETUP)
export const server = createLogger(namespaces.LOGGER_SERVER)
export const vite = createLogger(namespaces.LOGGER_VITE)
