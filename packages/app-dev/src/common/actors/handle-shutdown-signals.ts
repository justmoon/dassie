import wtfnode from "wtfnode"

import { createActor } from "@dassie/lib-reactive"

export const handleShutdownSignals = () =>
  createActor((sig) => {
    const onShutdown = () => {
      sig.reactor
        .dispose()
        .then(() => {
          setTimeout(() => {
            if (process.env["DASSIE_DEV_OPEN_HANDLES"]) {
              wtfnode.dump()
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
