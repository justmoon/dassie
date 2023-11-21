import { dump } from "wtfnode"

import { Reactor, createActor, createTopic } from "@dassie/lib-reactive"

const OPEN_HANDLES_GRACE_PERIOD = 500

export const ShutdownTopic = () => createTopic<void>()

export const HandleShutdownSignalsActor = (reactor: Reactor) =>
  createActor((sig) => {
    const onShutdown = () => {
      sig.reactor.use(ShutdownTopic).emit()

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
    if (process.report) process.report.reportOnSignal = true

    sig.onCleanup(() => {
      process.off("SIGTERM", onShutdown)
      process.off("SIGINT", onShutdown)
    })
  })
