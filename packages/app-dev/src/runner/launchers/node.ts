import { start } from "@xen-ilp/app-node"
import { createLogger } from "@xen-ilp/lib-logger"

export const logger = createLogger("xen:dev:launcher:node")

start()
