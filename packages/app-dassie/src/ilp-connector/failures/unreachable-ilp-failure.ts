import { BaseIlpFailure } from "./base-ilp-failure"

export class UnreachableIlpFailure extends BaseIlpFailure {
  readonly errorCode = "F02" as const
  readonly name = "UnreachableIlpFailure"
}
