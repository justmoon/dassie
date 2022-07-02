import startNode from "@xen-ilp/app-node"
import { createLogger } from "@xen-ilp/lib-logger"

export const logger = createLogger("xen:dev:launcher:node")

const start = async () => {
  await startNode()
}

start().catch((error) => logger.logError(error))
