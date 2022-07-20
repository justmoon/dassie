import { createLogger } from "@xen-ilp/lib-logger"

import type { AsyncDisposer } from "../reactor"

const logger = createLogger("xen:reactive:lifecycle-scope")

export class LifecycleScope {
  isDisposed = false
  private cleanupQueue: AsyncDisposer[] = []

  onCleanup(cleanupHandler: AsyncDisposer) {
    if (this.isDisposed) {
      // If the scope has already been disposed, run the cleanup handler immediately.
      Promise.resolve(cleanupHandler()).catch((error: unknown) => {
        logger.error("error in cleanup handler", { error })
      })
    } else {
      this.cleanupQueue.push(cleanupHandler)
    }
  }

  async dispose() {
    if (this.isDisposed) return

    this.isDisposed = true

    // Run cleanup handlers in reverse order
    let cleanupHandler: AsyncDisposer | undefined
    while ((cleanupHandler = this.cleanupQueue.pop())) {
      await cleanupHandler()
    }
  }
}
