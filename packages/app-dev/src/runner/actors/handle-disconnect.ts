import { Reactor, createActor } from "@dassie/lib-reactive"

export const HandleDisconnectActor = (reactor: Reactor) =>
  createActor(() => {
    process.on("disconnect", () => {
      reactor.lifecycle.dispose().catch((error: unknown) => {
        console.error("error while disposing reactor", { error })
      })
    })
  })
