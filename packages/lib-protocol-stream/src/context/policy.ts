export interface StreamPolicy {
  /**
   * The minimum concurrency level, i.e. how many packets can be in flight at
   * the same time even in the most congested situation.
   *
   * Must be at least 1. Strongly recommended to be set to 1.
   */
  readonly minimumConcurrency: number

  /**
   * The maximum concurrency level, i.e. how many packets can be in flight at
   * the same time if no congestion is detected.
   *
   * The main reason to have a limit is to avoid sending so many packets that
   * the sender is computationally overwhelmed. It may also be useful in case
   * there is a bug that causes the concurrency to increase without bound.
   */
  readonly maximumConcurrency: number

  /**
   * The additive increment part of the AIMD congestion control algorithm.
   *
   * Does NOT have to be an integer.
   */
  readonly concurrencyIncreaseIncrement: number
}

export const DEFAULT_POLICY: StreamPolicy = {
  minimumConcurrency: 1,
  maximumConcurrency: 100,
  concurrencyIncreaseIncrement: 1,
}