import type { EffectContext } from "@dassie/lib-reactive"

export const handleShutdownSignals = (sig: EffectContext) => {
  const onShutdown = () => {
    void sig.reactor.dispose()
  }

  process.on("SIGTERM", onShutdown)
  process.on("SIGINT", onShutdown)

  sig.onCleanup(() => {
    process.off("SIGTERM", onShutdown)
    process.off("SIGINT", onShutdown)
  })
}
