import { Failure } from "@dassie/lib-type-utils"

export default class InvalidAccountFailure extends Failure {
  readonly name = "InvalidAccountFailure"

  constructor(
    public readonly whichAccount: "debit" | "credit",
    public readonly accountPath: string,
  ) {
    super()
  }
}
