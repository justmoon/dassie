export type Disposer = () => void | PromiseLike<void>

export interface Scope {
  name: string
  isDisposed: boolean

  /**
   * Register a callback that will be run when this scope is disposed.
   *
   * @remarks
   *
   * The callback may be asynchronous.
   *
   * @param cleanupHandler - Callback that will be run when the scope is disposed.
   */
  onCleanup: (cleanupHandler: Disposer) => void

  /**
   * Unregister a callback that was previously registered with {@link DisposableScopeImplementation.onCleanup}.
   *
   * @param cleanupHandler - Callback to remove from the cleanup queue.
   */
  offCleanup: (cleanupHandler: Disposer) => void

  /**
   * An AbortSignal that will be aborted when the scope is disposed.
   */
  abortSignal: AbortSignal
}

export interface Disposable {
  /**
   * Dispose of this object. Calls cleanup handlers and marks the object as disposed.
   *
   * @remarks
   *
   * This method is idempotent.
   *
   * @returns A promise that resolves when all cleanup handlers have been run.
   */
  dispose: () => Promise<void>
}

export interface DisposableScope extends Scope, Disposable {}

export class DisposableScopeImplementation implements DisposableScope {
  constructor(public readonly name: string) {}

  /**
   * Set of cleanup handlers that will be run when the scope is disposed.
   *
   * If the scope is already disposed, this value will be undefined and the cleanup handlers will be run immediately.
   */
  private cleanupQueue: Disposer[] | undefined = []

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

  dispose = async () => {
    if (!this.cleanupQueue) return

    const cleanupQueue = this.cleanupQueue
    this.cleanupQueue = undefined

    // Run cleanup handlers in reverse order
    for (let index = cleanupQueue.length - 1; index >= 0; index--) {
      await cleanupQueue[index]!()
    }
  }

  private cachedAbortSignal: AbortSignal | undefined

  get abortSignal() {
    if (!this.cachedAbortSignal) {
      if (this.isDisposed) {
        this.cachedAbortSignal = AbortSignal.abort()
      } else {
        const abortController = new AbortController()
        this.onCleanup(() => {
          abortController.abort()
        })
        this.cachedAbortSignal = abortController.signal
      }
    }

    return this.cachedAbortSignal
  }
}

export const createScope = (name: string) =>
  new DisposableScopeImplementation(name)

/**
 * Make a scope within the confines of another scope.
 *
 * @remarks
 *
 * This simply means that the inner scope should be disposed automatically when
 * the outer scope is disposed.
 */
export const confineScope = (inner: DisposableScope, outer: Scope) => {
  outer.onCleanup(inner.dispose)
  inner.onCleanup(() => {
    outer.offCleanup(inner.dispose)
  })
}
