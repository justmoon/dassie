import type { Tagged } from "type-fest"

export type TimeoutId = Tagged<unknown, "TimeoutId">

export type IntervalId = Tagged<unknown, "IntervalId">

export interface Clock {
  /**
   * Returns the current time in milliseconds.
   *
   * This the equivalent of `Date.now()`.
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
}
