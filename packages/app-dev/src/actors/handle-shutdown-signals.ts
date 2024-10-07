import chalk from "chalk"
import { dump } from "wtfnode"

import process from "node:process"

import { type Reactor, createActor } from "@dassie/lib-reactive"

const OPEN_HANDLES_GRACE_PERIOD = 500

export const HandleShutdownSignalsActor = (reactor: Reactor) =>
  createActor((sig) => {
    const handleShutdown = () => {
      // eslint-disable-next-line no-console
      console.log(chalk.red("\n  Shutting down development server...\n"))

      // Immediately unregister signal handlers which enables the user to
      // forcefully quit the process by pressing ctrl+c again.
      process.off("SIGTERM", handleShutdown)
      process.off("SIGINT", handleShutdown)

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

    process.on("SIGTERM", handleShutdown)
    process.on("SIGINT", handleShutdown)
    process.report.reportOnSignal = true

    sig.onCleanup(() => {
      process.off("SIGTERM", handleShutdown)
      process.off("SIGINT", handleShutdown)
    })

    return {
      handleShutdown,
    }
  })
