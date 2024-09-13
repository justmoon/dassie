import { Failure } from "@dassie/lib-type-utils"

export class InvalidExchangeRateFailure extends Failure {
  readonly name = "InvalidExchangeRateFailure"

  constructor(readonly reason: string) {
    super()
  }
}

export const EXCHANGE_RATE_NEGATIVE_FAILURE = new InvalidExchangeRateFailure(
  "Exchange rate must be non-negative",
)
export const EXCHANGE_RATE_ZERO_FAILURE = new InvalidExchangeRateFailure(
  "Exchange rate must be non-zero",
)
