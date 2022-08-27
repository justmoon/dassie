import type { AsyncDisposer } from "../reactor"

export class LifecycleScope {
  isDisposed = false
  private cleanupQueue: AsyncDisposer[] = []

  onCleanup(cleanupHandler: AsyncDisposer) {
    if (this.isDisposed) {
      // If the scope has already been disposed, run the cleanup handler immediately.
      Promise.resolve(cleanupHandler()).catch((error: unknown) => {
        console.error("error in cleanup handler", { error })
      })
    } else {
      this.cleanupQueue.push(cleanupHandler)
    }
  }

  dispose = async () => {
    if (this.isDisposed) return

    this.isDisposed = true

    // Run cleanup handlers in reverse order
    let cleanupHandler: AsyncDisposer | undefined
    while ((cleanupHandler = this.cleanupQueue.pop())) {
      await cleanupHandler()
    }
  }

  deriveChildLifecycle(): LifecycleScope {
    const child = new LifecycleScope()
    this.onCleanup(child.dispose)
    return child
  }
}
