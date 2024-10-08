import { Failure } from "@dassie/lib-type-utils"

import type { BaseRequestContext } from "../types/context"
import type { HttpFailure } from "../types/http-failure"
import { getResponseOptionsFromContext } from "../utils/get-response-from-context"

export abstract class DefaultHttpFailure
  extends Failure
  implements HttpFailure
{
  abstract readonly statusCode: number

  constructor(readonly message: string) {
    super()
  }

  asResponse(context: BaseRequestContext) {
    const responseInit = getResponseOptionsFromContext(context)

    if (!responseInit.status) {
      responseInit.status = this.statusCode
    }

    return new Response(this.message, responseInit)
  }
}
