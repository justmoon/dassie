import { Failure } from "@dassie/lib-type-utils"

export interface IlpFailure extends Failure {
  readonly errorCode: string
  readonly message: string
}
