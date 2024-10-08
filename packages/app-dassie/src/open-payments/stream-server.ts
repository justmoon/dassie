import {
  Connection,
  DataAndMoneyStream,
  createServer,
} from "ilp-protocol-stream"

import { createActor } from "@dassie/lib-reactive"

import type * as dassieBase from "../base/types/dassie-base"
import { payment as logger } from "../logger/instances"
import { CreatePlugin } from "./functions/create-plugin"

export const StreamServerServiceActor = () =>
  createActor(async (sig: dassieBase.DassieActorContext) => {
    const createPlugin = sig.reactor.use(CreatePlugin)

    logger.debug?.("starting stream server")

    const plugin = createPlugin(sig.scope)

    const server = await createServer({
      plugin,
    })

    server.on("connection", (connection: Connection) => {
      connection.on("stream", (stream: DataAndMoneyStream) => {
        stream.setReceiveMax(Number.POSITIVE_INFINITY)
        stream.on("money", (amount: string) => {
          logger.debug?.("got money", { amount, stream: stream.id })
        })
        stream.on("end", () => {
          logger.debug?.("stream closed", { stream: stream.id })
        })
      })
    })

    return server
  })
