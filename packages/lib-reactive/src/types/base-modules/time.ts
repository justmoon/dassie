export interface TimeoutOptions {
  /**
   * An optional abort signal that will cancel the timeout.
   */
  signal?: AbortSignal
}

export interface Time {
  /**
   * Returns the current time in milliseconds.
   *
   * This is a wrapper around `Date.now()`.
   */
  now(): number

  /**
   * Returns a promise that resolves after the given delay.
   *
   * This is a wrapper around `setTimeout`.
   */
  timeout(delay: number, options?: TimeoutOptions): Promise<void>

  /**
   * Returns a promise that resolves as a new macrotask on the event loop.
   *
   * This is a wrapper around `setImmediate`.
   */
  immediate(): Promise<void>

  /**
   * Get an abort signal that will be aborted after the given delay.
   *
   * This is a wrapper around `AbortSignal.timeout`.
   *
   * @param delay - The delay in milliseconds.
   */
  abortTimeout(delay: number): AbortSignal
}
