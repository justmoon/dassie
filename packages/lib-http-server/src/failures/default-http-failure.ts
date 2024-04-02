import { Failure } from "@dassie/lib-type-utils"

import type { RequestContext } from "../context"
import { HttpFailure } from "../types/http-failure"

export abstract class DefaultHttpFailure
  extends Failure
  implements HttpFailure
{
  abstract readonly statusCode: number

  constructor(readonly message: string) {
    super()
  }

  asResponse({ headers }: RequestContext) {
    return new Response(this.message, {
      status: this.statusCode,
      headers,
    })
  }
}
