import { Failure } from "@dassie/lib-type-utils"

import { HttpResult } from "./http-result"

export interface HttpFailure extends Failure, HttpResult {
  readonly statusCode: number
  readonly message: string
}
