import { Failure } from "@dassie/lib-type-utils"

export default class InvalidLinkStateFailure extends Failure {
  readonly name = "InvalidLinkStateFailure"

  constructor(public readonly message: string) {
    super()
  }
}
