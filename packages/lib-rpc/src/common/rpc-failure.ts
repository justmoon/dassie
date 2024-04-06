import { Failure } from "@dassie/lib-type-utils"

export class RpcFailure extends Failure {
  name = "RpcFailure"

  constructor(public readonly message: string) {
    super()
  }
}
