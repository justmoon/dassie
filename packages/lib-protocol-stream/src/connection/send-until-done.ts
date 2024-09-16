import { createDeferred } from "@dassie/lib-reactive"

import { hasWorkToDo } from "./has-work-to-do"
import { sendOnce } from "./send-once"
import type { ConnectionState } from "./state"

export function sendUntilDone(state: ConnectionState) {
  // If there is already a send loop active, we don't need to start another one
  if (state.sendLoopPromise) return state.sendLoopPromise

  state.context.logger.debug?.("starting send loop")

  const promisePool = new Set<Promise<void>>()

  state.sendLoopPromise = (async function sendLoop() {
    for (;;) {
      const hasWork = hasWorkToDo(state)

      // If we have no more new work to do and no work is in progress, that
      // means we are fully done and can end the loop.
      if (!hasWork && promisePool.size === 0) break

      if (promisePool.size >= state.concurrency || !hasWork) {
        state.sendLoopWaker = createDeferred()

        // Check again when any of the current packets finishes or someone
        // wakes us up.
        await Promise.race([...promisePool, state.sendLoopWaker])
        continue
      }

      const promise = sendOnce(state)

      promisePool.add(promise)
      promise
        .catch((error: unknown) => {
          state.context.logger.error("error in sendOnce", { error })
        })
        .finally(() => {
          promisePool.delete(promise)
        })
    }
  })().catch((error: unknown) => {
    console.error("error in send loop", { error })
  })

  return state.sendLoopPromise
}
