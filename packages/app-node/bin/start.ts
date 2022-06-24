#!/usr/bin/env node
import { createLogger } from "@xen-ilp/lib-logger"

import startApp from "../src/index"

const logger = createLogger("xen:node")

startApp().catch((error) => logger.logError(error))
