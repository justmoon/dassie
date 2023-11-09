import { Request, Response } from "express"
import Negotiator from "negotiator"
import { Promisable } from "type-fest"

import { HttpFailure } from "./types/http-failure"
import { HttpResponse } from "./types/http-response"

type EmptyRequest = Request<object, unknown, unknown>

export type HttpRequestHandler<TAdditionalRequestFields = object> = (
  request: EmptyRequest & TAdditionalRequestFields,
  response: Response,
) => Promisable<HttpFailure | HttpResponse>

const handleError = (
  request: EmptyRequest,
  response: Response,
  error: unknown,
) => {
  const negotiator = new Negotiator(request)
  const mediaType = negotiator.mediaType(["application/json", "text/html"])

  console.error("error in http request handler", { error })

  switch (mediaType) {
    case "application/json": {
      response.status(500).json({
        error: "Internal Server Error",
      })
      break
    }
    case "text/html": {
      response.status(500).send(`<h1>Internal Server Error</h1>`)
      break
    }
    default: {
      response.status(500).send("Internal Server Error")
    }
  }
}
export const createHandler =
  <TAdditionalRequestFields = object>(
    handler: HttpRequestHandler<TAdditionalRequestFields>,
  ) =>
  (request: EmptyRequest & TAdditionalRequestFields, response: Response) => {
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
