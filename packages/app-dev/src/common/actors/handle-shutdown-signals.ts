import { dump } from "wtfnode"

import process from "node:process"

import { type Reactor, createActor } from "@dassie/lib-reactive"

const OPEN_HANDLES_GRACE_PERIOD = 500

export const HandleShutdownSignalsActor = (reactor: Reactor) =>
  createActor((sig) => {
    const onShutdown = () => {
      reactor
        .dispose()
        .then(() => {
          if (process.env["DASSIE_DEV_OPEN_HANDLES"]) {
            setTimeout(() => {
              dump()
            }, OPEN_HANDLES_GRACE_PERIOD)
          }
        })
        .catch((error: unknown) => {
          console.error("failed to dispose reactor", { error })
        })
    }

    process.on("SIGTERM", onShutdown)
    process.on("SIGINT", onShutdown)
    process.report.reportOnSignal = true

    sig.onCleanup(() => {
      process.off("SIGTERM", onShutdown)
      process.off("SIGINT", onShutdown)
    })
  })
