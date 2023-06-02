import type { AsyncDisposer } from "../reactor"

export interface Lifecycle {
  name: string
  isDisposed: boolean
  onCleanup: typeof DisposableLifecycle.prototype.onCleanup
  offCleanup: typeof DisposableLifecycle.prototype.offCleanup
}

export class DisposableLifecycle implements Lifecycle {
  constructor(public readonly name: string) {}

  /**
   * Set of cleanup handlers that will be run when the scope is disposed.
   *
   * If the scope is already disposed, this value will be undefined and the cleanup handlers will be run immediately.
   */
  private cleanupQueue: AsyncDisposer[] | undefined = []

  /**
   * Register a callback that will be run when this lifecycle scope is disposed.
   *
   * @remarks
   *
   * The callback may be asynchronous.
   *
   * @param cleanupHandler - Callback that will be run when the scope is disposed.
   */
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

  /**
   * Unregister a callback that was previously registered with {@link DisposableLifecycle.onCleanup}.
   *
   * @param cleanupHandler - Callback to remove from the cleanup queue.
   */
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

  /**
   * Call the cleanup handlers and mark this lifecycle as disposed.
   *
   * @remarks
   *
   * This method is idempotent.
   *
   * @returns A promise that resolves when all cleanup handlers have been run.
   */
  dispose = async () => {
    if (!this.cleanupQueue) return

    const cleanupQueue = this.cleanupQueue
    this.cleanupQueue = undefined

    // Run cleanup handlers in reverse order
    for (let index = cleanupQueue.length - 1; index >= 0; index--) {
      await cleanupQueue[index]!()
    }
  }

  /**
   * Make this lifecycle exist within the confines of another lifecycle.
   *
   * @remarks
   *
   * This simply means that this lifecycle should be disposed automatically when the parent lifecycle is disposed.
   */
  attachToParent = (parent: Lifecycle) => {
    parent.onCleanup(this.dispose)
    this.onCleanup(() => parent.offCleanup(this.dispose))
  }
}
