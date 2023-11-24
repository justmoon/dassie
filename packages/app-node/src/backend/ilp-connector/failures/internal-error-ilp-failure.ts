import { BaseIlpFailure } from "./base-ilp-failure"

export class InternalErrorIlpFailure extends BaseIlpFailure {
  readonly errorCode = "T00" as const
  readonly name = "InternalErrorIlpFailure"
}
