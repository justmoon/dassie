import { createRequire } from "node:module"

import { createActor } from "@dassie/lib-reactive"

import { TrpcClientServiceActor } from "../services/trpc-client"

export const PatchIlpLoggerActor = () =>
  createActor((sig) => {
    const trpcClient = sig.readAndTrack(TrpcClientServiceActor)
    if (!trpcClient) return

    const ownRequire = createRequire(import.meta.url)
    const nodeRequire = createRequire(ownRequire.resolve("@dassie/app-node"))
    const streamRequire = createRequire(
      nodeRequire.resolve("ilp-protocol-stream"),
    )
    const loggerRequire = createRequire(streamRequire.resolve("ilp-logger"))
    const debug = loggerRequire("debug") as unknown
    ;(debug as { log: typeof console.debug }).log = (
      message: string,
      ...parameters: unknown[]
    ) => {
      const firstSpace = message.indexOf(" ")
      const namespace =
        firstSpace === -1 ? "debug" : message.slice(0, firstSpace)

      void trpcClient.runner.notifyLogLine.mutate([
        process.env["DASSIE_DEV_NODE_ID"] ?? "unknown",
        {
          type: "debug",
          namespace,
          date: Date.now(),
          message: firstSpace === -1 ? message : message.slice(firstSpace + 1),
          parameters,
        },
      ])
    }
  })
