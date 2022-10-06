import type { EffectContext } from "@dassie/lib-reactive"

export const handleDisconnect = (sig: EffectContext) => {
  process.on("disconnect", () => {
    sig.reactor.dispose().catch((error: unknown) => {
      console.error("error while disposing reactor", { error })
    })
  })
}
