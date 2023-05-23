import {
  Connection,
  DataAndMoneyStream,
  createServer,
} from "ilp-protocol-stream"

import { createLogger } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

import { nodeIlpAddressSignal } from "../ilp-connector/computed/node-ilp-address"
import { createPlugin } from "./utils/create-plugin"

const logger = createLogger("das:node:stream-server")

export const streamServerService = () =>
  createActor(async (sig) => {
    const nodeIlpAddress = sig.get(nodeIlpAddressSignal)

    logger.debug("starting stream server")

    const plugin = createPlugin(sig.reactor, nodeIlpAddress)

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
