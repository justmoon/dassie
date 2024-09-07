import type { Clock } from "../types/base-modules/clock"

/**
 * Get an abort signal that will be aborted after the given delay.
 *
 * This is a replacement for `AbortSignal.timeout` with support for passing in
 * a clock.
 *
 * @param clock - An instance of a Clock to manage the timeout.
 * @param delay - The delay in milliseconds.
 * @returns Abort signal which aborts after the given delay.
 */
export function abortTimeout(clock: Clock, delay: number): AbortSignal {
  const controller = new AbortController()

  clock.setTimeout(() => {
    controller.abort()
  }, delay)

  return controller.signal
}
