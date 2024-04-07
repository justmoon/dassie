import { Failure } from "@dassie/lib-type-utils"

import type { RequestContext } from "../types/context"
import { HttpFailure } from "../types/http-failure"
import { getResponseOptionsFromContext } from "../utils/get-response-from-context"

export abstract class DefaultHttpFailure
  extends Failure
  implements HttpFailure
{
  abstract readonly statusCode: number

  constructor(readonly message: string) {
    super()
  }

  asResponse(context: RequestContext) {
    const responseInit = getResponseOptionsFromContext(context)

    if (!responseInit.status) {
      responseInit.status = this.statusCode
    }

    return new Response(this.message, responseInit)
  }
}
