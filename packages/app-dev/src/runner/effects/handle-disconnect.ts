import { createActor } from "@dassie/lib-reactive"

export const handleDisconnect = () =>
  createActor((sig) => {
    process.on("disconnect", () => {
      sig.reactor.dispose().catch((error: unknown) => {
        console.error("error while disposing reactor", { error })
      })
    })
  })
