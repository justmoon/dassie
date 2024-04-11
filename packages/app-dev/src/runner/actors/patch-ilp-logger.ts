import { createRequire } from "node:module"

import { LogsStore } from "@dassie/app-node/src/common/stores/logs"
import { type Reactor, createActor } from "@dassie/lib-reactive"

export const PatchIlpLoggerActor = (reactor: Reactor) => {
  const logsStore = reactor.use(LogsStore)

  return createActor(() => {
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

      logsStore.addLogLine({
        type: "debug",
        namespace,
        node: process.env["DASSIE_DEV_NODE_ID"] ?? "unknown",
        date: Date.now(),
        message: firstSpace === -1 ? message : message.slice(firstSpace + 1),
        parameters,
        caller: undefined,
      })
    }
  })
}
