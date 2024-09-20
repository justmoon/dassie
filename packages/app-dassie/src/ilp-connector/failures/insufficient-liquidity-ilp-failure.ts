import { BaseIlpFailure } from "./base-ilp-failure"

export class InsufficientLiquidityIlpFailure extends BaseIlpFailure {
  readonly errorCode = "T04" as const
  readonly name = "InsufficientLiquidityIlpFailure"
}
