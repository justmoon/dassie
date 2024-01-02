import Negotiator from "negotiator"
import { Promisable } from "type-fest"

import { IncomingMessage, ServerResponse } from "node:http"

import { HttpFailure } from "./types/http-failure"
import { HttpResponse } from "./types/http-response"

export type HttpRequestHandler<TAdditionalRequestFields = object> = (
  request: IncomingMessage & TAdditionalRequestFields,
  response: ServerResponse,
) => Promisable<HttpFailure | HttpResponse>

const handleError = (
  request: IncomingMessage,
  response: ServerResponse,
  error: unknown,
) => {
  const negotiator = new Negotiator(request)
  const mediaType = negotiator.mediaType(["application/json", "text/html"])

  console.error("error in http request handler", { error })

  switch (mediaType) {
    case "application/json": {
      response.statusCode = 500
      response.write(JSON.stringify({ error: "Internal Server Error" }))
      break
    }
    case "text/html": {
      response.statusCode = 500
      response.write("<h1>Internal Server Error</h1>")
      break
    }
    default: {
      response.statusCode = 500
      response.write("Internal Server Error")
    }
  }
}
export const createHandler =
  <TAdditionalRequestFields = object>(
    handler: HttpRequestHandler<TAdditionalRequestFields>,
  ) =>
  (
    request: IncomingMessage & TAdditionalRequestFields,
    response: ServerResponse<IncomingMessage & TAdditionalRequestFields>,
  ) => {
    try {
      Promise.resolve(handler(request, response))
        .then((result) => {
          result.applyTo(response)
        })
        .catch((error) => {
          handleError(request, response, error)
        })
    } catch (error) {
      handleError(request, response, error)
    }
  }
