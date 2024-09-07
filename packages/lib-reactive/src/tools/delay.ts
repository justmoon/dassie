import { Failure } from "@dassie/lib-type-utils"

import { createDeferred } from "../deferred"
import type { Clock } from "../types/base-modules/clock"

export class AbortFailure extends Failure {
  readonly name = "AbortFailure"
  readonly message = "Operation aborted due to abort signal"
}

const ABORT_FAILURE = new AbortFailure()

/**
 * Provides a promise that resolves after the given duration.
 *
 * @param clock - An instance of a Clock that can manage the timeout.
 * @param durationMilliseconds - The duration in milliseconds.
 * @returns A promise that resolves after the given duration.
 */
export function delay(
  clock: Clock,
  durationMilliseconds: number,
): Promise<void> {
  const deferred = createDeferred()

  clock.setTimeout(() => {
    deferred.resolve()
  }, durationMilliseconds)

  return deferred
}

/**
 * Provides a promise that resolves either after the given duration or when the
 * abort signal is triggered. The promise will resolve with an `AbortFailure`
 * if the abort signal is triggered or `undefined` if the timeout is reached.
 *
 * @param clock - An instance of a Clock that can manage the timeout.
 * @param durationMilliseconds - The duration in milliseconds.
 * @param signal - The abort signal which will abort the timeout.
 * @returns
 */
export function delayWithAbortSignal(
  clock: Clock,
  durationMilliseconds: number,
  signal: AbortSignal,
) {
  const deferred = createDeferred<AbortFailure | void>()

  const timeoutId = clock.setTimeout(() => {
    deferred.resolve()
  }, durationMilliseconds)

  signal.addEventListener("abort", () => {
    clock.clearTimeout(timeoutId)
    deferred.resolve(ABORT_FAILURE)
  })

  return deferred
}
