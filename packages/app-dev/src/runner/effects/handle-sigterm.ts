import type { EffectContext } from "@xen-ilp/lib-reactive"

export const handleSigterm = (sig: EffectContext) => {
  const onSigterm = () => {
    void sig.reactor.dispose()
  }

  process.on("SIGTERM", onSigterm)

  sig.onCleanup(() => {
    process.off("SIGTERM", onSigterm)
  })
}
