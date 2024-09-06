import type { Logger } from "../logger"

export class AssertionError extends Error {
  constructor(message: string) {
    super(message)
    this.name = "AssertionError"
  }
}

export function assert(
  logger: Logger,
  condition: boolean,
  message: string,
): asserts condition {
  if (!condition) {
    logger.error(`unmet assertion: ${message}`)
    throw new AssertionError(message)
  }
}
