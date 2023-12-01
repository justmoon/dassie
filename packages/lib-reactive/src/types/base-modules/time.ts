import { Opaque } from "type-fest"

export interface TimeoutOptions {
  /**
   * An optional abort signal that will cancel the timeout.
   */
  signal?: AbortSignal
}

export type TimeoutId = Opaque<unknown, "TimeoutId">

export type IntervalId = Opaque<unknown, "IntervalId">

export interface Time {
  /**
   * Returns the current time in milliseconds.
   *
   * This is a wrapper around `Date.now()`.
   */
  now(): number

  /**
   * Equivalent of JavaScript's native `setTimeout`.
   */
  setTimeout(callback: () => void, delay: number): TimeoutId

  /**
   * Equivalent of JavaScript's native `clearTimeout`.
   */
  clearTimeout(id: TimeoutId): void

  /**
   * Equivalent of JavaScript's native `setInterval`.
   */
  setInterval(callback: () => void, delay: number): IntervalId

  /**
   * Equivalent of JavaScript's native `clearInterval`.
   */
  clearInterval(id: IntervalId): void

  /**
   * Returns a promise that resolves after the given delay.
   */
  timeout(delay: number, options?: TimeoutOptions): Promise<void>

  /**
   * Returns a promise that resolves as a new macrotask on the event loop.
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
