import { dump } from "wtfnode"

import { Reactor, createActor } from "@dassie/lib-reactive"

export const HandleShutdownSignalsActor = (reactor: Reactor) =>
  createActor((sig) => {
    const onShutdown = () => {
      reactor.lifecycle
        .dispose()
        .then(() => {
          setTimeout(() => {
            if (process.env["DASSIE_DEV_OPEN_HANDLES"]) {
              dump()
            }
          })
        })
        .catch((error: unknown) => {
          console.error("failed to dispose reactor", { error })
        })
    }

    process.on("SIGTERM", onShutdown)
    process.on("SIGINT", onShutdown)
    if (process.report) process.report.reportOnSignal = true

    sig.onCleanup(() => {
      process.off("SIGTERM", onShutdown)
      process.off("SIGINT", onShutdown)
    })
  })
