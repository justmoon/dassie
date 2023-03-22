import {
  Connection,
  DataAndMoneyStream,
  createServer,
} from "ilp-protocol-stream"

import { createLogger } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

import { primaryIlpAddressSignal } from "../ilp-connector/signals/primary-ilp-address"
import { createPlugin } from "./utils/create-plugin"

const logger = createLogger("das:node:stream-server")

export const streamServerService = () =>
  createActor(async (sig) => {
    const nodeIlpAddress = sig.get(primaryIlpAddressSignal)
    if (!nodeIlpAddress) return

    const { ilpAddress, plugin } = sig.run(createPlugin, nodeIlpAddress)

    logger.debug("starting stream server", { address: ilpAddress })

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
