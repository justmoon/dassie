import type { Response } from "express"

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

  applyTo(response: Response) {
    response.status(this.statusCode).send(this.message)
  }
}
