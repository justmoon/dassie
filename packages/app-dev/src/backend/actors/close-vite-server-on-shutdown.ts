import { createActor } from "@dassie/lib-reactive"

import { ShutdownTopic } from "../../common/actors/handle-shutdown-signals"
import { ViteServer } from "../unconstructables/vite-server"

/**
 * Responsible for closing the Vite server when the app is shutting down.
 *
 * @remarks
 *
 * The reason we can't just use the `reactor.onCleanup` callback is that we have a feature where the development server
 * is restarted when a file changes. The Vite server persists across these relaunches, meaning it survives the reactor.
 *
 * However, if the app is shutting down, we want to close the Vite server so that the process may exit. This actor is
 * responsible for that.
 */
export const CloseViteServerOnShutdownActor = () =>
  createActor((sig) => {
    sig.on(ShutdownTopic, () => {
      sig.reactor.onCleanup(async () => {
        await sig.reactor.use(ViteServer).close()
      })
    })
  })
