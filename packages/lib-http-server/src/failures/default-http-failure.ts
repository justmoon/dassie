import { ServerResponse } from "node:http"

import { Failure } from "@dassie/lib-type-utils"

import { HttpFailure } from "../types/http-failure"

export abstract class DefaultHttpFailure
  extends Failure
  implements HttpFailure
{
  abstract readonly statusCode: number

  constructor(readonly message: string) {
    super()
  }

  applyTo(response: ServerResponse) {
    response.statusCode = this.statusCode
    response.end(this.message)
  }
}
