import { AnyZodObject, infer as InferZodType } from "zod"

import { BadRequestFailure } from "../failures/bad-request-failure"
import { Middleware } from "../router"

export const parseQueryParameters =
  <TSchema extends AnyZodObject>(
    schema: TSchema,
  ): Middleware<object, { query: InferZodType<TSchema> }> =>
  (request) => {
    // Get query string from URL
    const query = Object.fromEntries(request.url.searchParams.entries())
    try {
      return { query: schema.parse(query) }
    } catch (error) {
      console.warn("invalid api request query string", {
        error,
      })
      return new BadRequestFailure("Invalid API request query string")
    }
  }
