import { Failure } from "@dassie/lib-type-utils"

export class AmountExceedsMaximumReceiveAmountFailure extends Failure {
  readonly name = "AmountExceedsMaximumReceiveAmountFailure"
}

export const AMOUNT_EXCEEDS_MAXIMUM_RECEIVE_AMOUNT =
  new AmountExceedsMaximumReceiveAmountFailure()
