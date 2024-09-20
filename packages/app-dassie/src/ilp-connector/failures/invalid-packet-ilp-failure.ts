import { BaseIlpFailure } from "./base-ilp-failure"

export class InvalidPacketIlpFailure extends BaseIlpFailure {
  readonly errorCode = "F01" as const
  readonly name = "InvalidPacketIlpFailure"
}
