import { Failure } from "@dassie/lib-type-utils"

export class ProbingUnsuccessfulFailure extends Failure {
  readonly name = "ProbingUnsuccessfulFailure"
}

export const PROBING_UNSUCCESSFUL_FAILURE = new ProbingUnsuccessfulFailure()
