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
  /**
   * The multiplicative decrease part of the AIMD congestion control algorithm.
   *
   * Must be between 0 and 1.
   */
  readonly concurrencyDecreaseFactor: number

  /**
   * How long to try to notify the other side that we are closing the connection.
   */
  readonly closeTimeout: number

  /**
   * How many times to retry sending a close packet.
   */
  readonly closeRetries: number

  /**
   * Cutoff below which we will treat an amount as zero.
   *
   * This is helpful because it is impossible to send very small amounts of
   * money precisely due to rounding.
   */
  readonly deMinimisAmount: bigint
}

export const DEFAULT_POLICY: StreamPolicy = {
  minimumConcurrency: 1,
  maximumConcurrency: 100,
  concurrencyIncreaseIncrement: 1,
  concurrencyDecreaseFactor: 0.5,
  closeTimeout: 30_000,
  closeRetries: 3,
  deMinimisAmount: 1000n,
}
