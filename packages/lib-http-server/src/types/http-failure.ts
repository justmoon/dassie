import type { Response } from "express"

import { Failure } from "@dassie/lib-type-utils"

export interface HttpFailure extends Failure {
  readonly statusCode: number
  readonly message: string

  applyTo(response: Response): void
}
