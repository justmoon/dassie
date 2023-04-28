import type { AsyncDisposer } from "../reactor"

export class LifecycleScope {
  /**
   * Set of cleanup handlers that will be run when the scope is disposed.
   *
   * If the scope is already disposed, this value will be undefined and the cleanup handlers will be run immediately.
   */
  private cleanupQueue: AsyncDisposer[] | undefined = []

  onCleanup = (cleanupHandler: AsyncDisposer) => {
    if (!this.cleanupQueue) {
      // If the scope has already been disposed, run the cleanup handler immediately.
      Promise.resolve(cleanupHandler()).catch((error: unknown) => {
        console.error("error in cleanup handler", { error })
      })
      return
    }

    this.cleanupQueue.push(cleanupHandler)
  }

  offCleanup = (cleanupHandler: AsyncDisposer) => {
    // TODO: This could be slow if there are a lot of cleanup handlers. Benchmark and optimize for 0, 1, small, and large numbers of handlers.
    const index = this.cleanupQueue?.indexOf(cleanupHandler) ?? -1
    if (index !== -1) {
      this.cleanupQueue!.splice(index, 1)
    }
  }

  get isDisposed() {
    return this.cleanupQueue === undefined
  }

  dispose = async () => {
    if (!this.cleanupQueue) return

    const cleanupQueue = this.cleanupQueue
    this.cleanupQueue = undefined

    // Run cleanup handlers in reverse order
    for (let index = cleanupQueue.length - 1; index >= 0; index--) {
      await cleanupQueue[index]!()
    }
  }

  deriveChildLifecycle(): LifecycleScope {
    if (!this.cleanupQueue) {
      throw new Error("cannot derive child lifecycle from disposed scope")
    }

    const child = new LifecycleScope()
    this.onCleanup(child.dispose)
    child.onCleanup(() => this.offCleanup(child.dispose))
    return child
  }
}
