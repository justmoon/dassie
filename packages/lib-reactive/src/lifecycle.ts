export type Disposer = () => void | PromiseLike<void>

export interface LifecycleScope {
  name: string
  isDisposed: boolean
  onCleanup: (cleanupHandler: Disposer) => void
  offCleanup: (cleanupHandler: Disposer) => void
}

export interface DisposableLifecycleScope extends LifecycleScope {
  dispose: () => Promise<void>
  confineTo: (parent: LifecycleScope) => void
}

export class DisposableLifecycleScopeImplementation
  implements DisposableLifecycleScope
{
  constructor(public readonly name: string) {}

  /**
   * Set of cleanup handlers that will be run when the scope is disposed.
   *
   * If the scope is already disposed, this value will be undefined and the cleanup handlers will be run immediately.
   */
  private cleanupQueue: Disposer[] | undefined = []

  /**
   * Register a callback that will be run when this lifecycle scope is disposed.
   *
   * @remarks
   *
   * The callback may be asynchronous.
   *
   * @param cleanupHandler - Callback that will be run when the scope is disposed.
   */
  onCleanup = (cleanupHandler: Disposer) => {
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
   * Unregister a callback that was previously registered with {@link DisposableLifecycleScopeImplementation.onCleanup}.
   *
   * @param cleanupHandler - Callback to remove from the cleanup queue.
   */
  offCleanup = (cleanupHandler: Disposer) => {
    if (!this.cleanupQueue) return

    // TODO: This could be slow if there are a lot of cleanup handlers.
    const index = this.cleanupQueue.indexOf(cleanupHandler)
    if (index !== -1) {
      this.cleanupQueue.splice(index, 1)
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
  confineTo = (parent: LifecycleScope) => {
    parent.onCleanup(this.dispose)
    this.onCleanup(() => parent.offCleanup(this.dispose))
  }
}

export const createLifecycleScope = (name: string) =>
  new DisposableLifecycleScopeImplementation(name)
