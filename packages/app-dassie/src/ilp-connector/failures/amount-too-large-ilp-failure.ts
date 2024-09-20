import { BaseIlpFailure } from "./base-ilp-failure"

export class AmountTooLargeIlpFailure extends BaseIlpFailure {
  readonly errorCode = "F08" as const
  readonly name = "AmountTooLargeIlpFailure"
}
