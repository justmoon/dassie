import wtfnode from "wtfnode"

import type { EffectContext } from "@dassie/lib-reactive"

export const handleShutdownSignals = (sig: EffectContext) => {
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

  sig.onCleanup(() => {
    process.off("SIGTERM", onShutdown)
    process.off("SIGINT", onShutdown)
  })
}
