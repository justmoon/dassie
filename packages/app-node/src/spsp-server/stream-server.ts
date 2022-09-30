import {
  Connection,
  DataAndMoneyStream,
  createServer,
} from "ilp-protocol-stream"

import { createLogger } from "@dassie/lib-logger"
import { createService } from "@dassie/lib-reactive"

import { createPlugin } from "./utils/create-plugin"

const logger = createLogger("das:node:stream-server")

export const streamServerService = () =>
  createService(async (sig) => {
    const plugin = sig.run(createPlugin)

    logger.debug("starting stream server", { address: plugin.ilpAddress })

    const server = await createServer({
      plugin,
    })

    server.on("connection", (connection: Connection) => {
      connection.on("stream", (stream: DataAndMoneyStream) => {
        stream.setReceiveMax(Number.POSITIVE_INFINITY)
        stream.on("money", (amount: string) => {
          logger.debug("got money", { amount, stream: stream.id })
        })
        stream.on("end", () => {
          logger.debug("stream closed", { stream: stream.id })
        })
      })
    })

    return server
  })
