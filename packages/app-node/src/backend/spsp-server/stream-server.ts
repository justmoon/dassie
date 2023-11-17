import {
  Connection,
  DataAndMoneyStream,
  createServer,
} from "ilp-protocol-stream"

import { createActor } from "@dassie/lib-reactive"

import { payment as logger } from "../logger/instances"
import { ManagePluginsActor } from "./manage-plugins"

export const StreamServerServiceActor = () =>
  createActor(async (sig) => {
    const pluginManager = sig.reactor.use(ManagePluginsActor)

    logger.debug("starting stream server")

    const plugin = await pluginManager.api.createPlugin.ask()

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
