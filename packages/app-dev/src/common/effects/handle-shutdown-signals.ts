import type { EffectContext } from "@dassie/lib-reactive"

export const handleShutdownSignals = (sig: EffectContext) => {
  const onShutdown = () => {
    sig.reactor.dispose().catch((error: unknown) => {
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
