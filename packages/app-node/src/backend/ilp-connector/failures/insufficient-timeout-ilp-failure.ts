import { BaseIlpFailure } from "./base-ilp-failure"

export class InsufficientTimeoutIlpFailure extends BaseIlpFailure {
  readonly errorCode = "R02" as const
  readonly name = "InsufficientTimeoutIlpFailure"
}
