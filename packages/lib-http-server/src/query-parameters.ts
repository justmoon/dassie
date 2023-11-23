import { AnyZodObject, infer as InferZodType } from "zod"

import { BadRequestFailure } from "./failures/bad-request-failure"
import { Middleware } from "./router"

export const parseQueryParameters =
  <TSchema extends AnyZodObject>(
    schema: TSchema,
  ): Middleware<object, { query: InferZodType<TSchema> }> =>
  (request) => {
    try {
      return { query: schema.parse(request.query) }
    } catch (error) {
      console.warn("invalid api request query string", {
        error,
      })
      return new BadRequestFailure("Invalid API request query string")
    }
  }
