import type { RequestContext } from "../types/context"
import type { HttpResponse } from "../types/http-response"
import { getResponseOptionsFromContext } from "../utils/get-response-from-context"

export class RedirectResponse implements HttpResponse {
  constructor(
    private readonly location: string,
    private readonly statusCode: number = 302,
  ) {}

  asResponse(context: RequestContext) {
    const { response } = context

    response.headers.set("Location", this.location)

    return new Response(null, {
      status: this.statusCode,
      ...getResponseOptionsFromContext(context),
    })
  }
}
