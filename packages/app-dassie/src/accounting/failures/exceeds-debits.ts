import { Failure } from "@dassie/lib-type-utils"

export default class ExceedsDebitsFailure extends Failure {
  readonly name = "ExceedsDebitsFailure"
}

export const EXCEEDS_DEBITS_FAILURE = new ExceedsDebitsFailure()
