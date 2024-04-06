/**
 * Interface for configuring exponential backoff algorithm.
 */
export interface ExponentialBackoffParameters {
  /**
   * Whether the first attempt should happen immediately without any backoff. Defaults to true.
   */
  immediateFirstAttempt?: boolean

  /**
   * The minimum amount of time to delay before the next retry. In milliseconds. Defaults to 1s.
   */
  minDelay?: number

  /**
   * The maximum amount of time to delay before the next retry. In milliseconds. Defaults to 30s.
   */
  maxDelay?: number

  /**
   * The factor by which the delay should increase after each retry. Optional.
   */
  factor?: number

  /**
   * A value to add randomness to the delay to prevent a thundering herd problem. Optional.
   */
  jitter?: number
}

const DEFAULT_EXPONENTIAL_BACKOFF_PARAMETERS: Required<ExponentialBackoffParameters> =
  {
    immediateFirstAttempt: true,
    minDelay: 1000,
    maxDelay: 30_000,
    factor: 2,
    jitter: 0.5,
  }

/**
 * Calculate delay for exponential backoff.
 *
 * @param attempt - The reconnection attempt number, starting from 0. Can be set to a negative number to skip the backoff.
 */
export function createExponentialBackoffCalculator(
  options: ExponentialBackoffParameters = {},
) {
  const { immediateFirstAttempt, minDelay, maxDelay, factor, jitter } = {
    ...DEFAULT_EXPONENTIAL_BACKOFF_PARAMETERS,
    ...options,
  }

  return function calculateExponentialBackoff(attempt: number) {
    if (immediateFirstAttempt) {
      attempt -= 1
    }

    if (attempt <= -1) return 0

    const delay = Math.min(minDelay * Math.pow(factor, attempt), maxDelay)
    const randomization = delay * jitter * (Math.random() * 2 - 1) // Random value between -jitter * delay and +jitter * delay
    return Math.max(minDelay, delay + randomization)
  }
}
