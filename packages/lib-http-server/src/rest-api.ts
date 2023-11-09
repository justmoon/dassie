import type { Request, Response, Router } from "express"
import { json } from "express"
import type { AnyZodObject, infer as inferZodType } from "zod"

import { BadRequestFailure, HttpRequestHandler, createHandler } from "."

export const HTTP_METHODS = [
  "get",
  "post",
  "put",
  "delete",
  "patch",
  "head",
  "options",
] as const

export type HttpMethod = (typeof HTTP_METHODS)[number]

export type RestApiBuilder = {
  [method in HttpMethod]: (path: string) => ApiRouteBuilder<ApiRouteParameters>
}

export type ApiRequest<TParameters extends ApiRouteParameters> = Request<
  object,
  unknown,
  inferZodType<TParameters["bodySchema"]>,
  inferZodType<TParameters["querySchema"]>
>

export type ApiHandler<TParameters extends ApiRouteParameters> =
  HttpRequestHandler<ApiRequest<TParameters>>

export interface ApiRouteBuilder<TParameters extends ApiRouteParameters> {
  /**
   * Provide a JSON validator for the request body.
   */
  body: <TBodySchema extends AnyZodObject>(
    schema: TBodySchema,
  ) => ApiRouteBuilder<{
    bodySchema: TBodySchema
    querySchema: TParameters["querySchema"]
  }>

  /**
   * Provide a validator for the query string record.
   */
  query: <TQuerySchema extends AnyZodObject>(
    schema: TQuerySchema,
  ) => ApiRouteBuilder<{
    bodySchema: TParameters["bodySchema"]
    querySchema: TQuerySchema
  }>

  /**
   * Enable CORS for this route.
   */
  cors: () => ApiRouteBuilder<TParameters>

  /**
   * Provide the handler for the API route.
   */
  handler: <THandler extends ApiHandler<TParameters>>(
    handler: THandler,
  ) => ApiRouteInstance
}

export interface ApiRouteParameters {
  bodySchema: AnyZodObject
  querySchema: AnyZodObject
}

export interface ApiRouteInstance {
  dispose: () => void
}

export const createRestApi = (router: Router) => {
  const jsonHandler = json()

  const createBuilder = (method: HttpMethod) => (path: string) => {
    let bodyValidator: AnyZodObject | undefined
    let queryValidator: AnyZodObject | undefined
    let cors = false
    let userHandler: ApiHandler<ApiRouteParameters> | undefined

    const routeHandler = async (request: Request, response: Response) => {
      if (!userHandler) {
        throw new Error("No handler provided for API route")
      }

      try {
        bodyValidator?.parse(request.body)
      } catch (error) {
        console.warn("invalid api request body", { path, error })
        return new BadRequestFailure("Invalid API request body")
      }

      try {
        queryValidator?.parse(request.query)
      } catch (error) {
        console.warn("invalid api request query string", { path, error })
        return new BadRequestFailure("Invalid API request query string")
      }

      if (cors) {
        response.setHeader("Access-Control-Allow-Origin", "*")
      }

      return userHandler(request, response)
    }

    const routeDescriptor: ApiRouteBuilder<ApiRouteParameters> = {
      body: (schema) => {
        bodyValidator = schema
        return routeDescriptor
      },
      query: (schema) => {
        queryValidator = schema
        return routeDescriptor
      },
      cors: () => {
        cors = true
        return routeDescriptor
      },
      handler: (handler) => {
        userHandler = handler

        router[method](path, jsonHandler, createHandler(routeHandler))

        return {
          dispose: () => {
            const routeIndex = router.stack.findIndex(
              (layer: {
                route?: {
                  path: string
                  stack: { handle: HttpRequestHandler }[]
                }
              }) => {
                return (
                  layer.route?.path === path &&
                  layer.route.stack.findIndex(
                    (sublayer) => sublayer.handle === routeHandler,
                  ) !== -1
                )
              },
            )
            if (routeIndex !== -1) router.stack.splice(routeIndex, 1)
          },
        }
      },
    }

    return routeDescriptor
  }

  return Object.fromEntries(
    HTTP_METHODS.map((method) => [method, createBuilder(method)] as const),
  ) as RestApiBuilder
}
