import { Failure } from "@dassie/lib-type-utils"

export default class ExceedsCreditsFailure extends Failure {
  readonly name = "ExceedsCreditsFailure"
}

export const EXCEEDS_CREDITS_FAILURE = new ExceedsCreditsFailure()
