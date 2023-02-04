import type { NextFunction, Request, Response, Router } from "express"
import { json } from "express"
import type { Promisable } from "type-fest"
import type { AnyZodObject, infer as inferZodType } from "zod"

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

export interface ApiRequest<TParameters extends ApiRouteParameters>
  extends Request {
  body: inferZodType<TParameters["bodySchema"]>
  query: inferZodType<TParameters["querySchema"]>
}

export type ApiHandler<TParameters extends ApiRouteParameters> = (
  request: ApiRequest<TParameters>,
  response: Response
) => Promisable<void>

export interface ApiRouteBuilder<TParameters extends ApiRouteParameters> {
  /**
   * Provide a JSON validator for the request body.
   */
  body: <TBodySchema extends AnyZodObject>(
    schema: TBodySchema
  ) => ApiRouteBuilder<{
    bodySchema: TBodySchema
    querySchema: TParameters["querySchema"]
  }>

  /**
   * Provide a validator for the query string record.
   */
  query: <TQuerySchema extends AnyZodObject>(
    schema: TQuerySchema
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
    handler: THandler
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

    const routeHandler = (
      request: Request,
      response: Response,
      next: NextFunction
    ) => {
      if (!userHandler) {
        throw new Error("No handler provided for API route")
      }

      try {
        bodyValidator?.parse(request.body)
      } catch (error) {
        console.warn("invalid api request body", { path, error })
        response.status(400).json(error)
        return
      }

      try {
        queryValidator?.parse(request.query)
      } catch (error) {
        console.warn("invalid api request query string", { path, error })
        response.status(400).json(error)
        return
      }

      if (cors) {
        response.setHeader("Access-Control-Allow-Origin", "*")
      }

      Promise.resolve(userHandler(request, response)).then(next).catch(next)
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

        router[method](path, jsonHandler, routeHandler)

        return {
          dispose: () => {
            const routeIndex = router.stack.findIndex(
              (layer: {
                route?: { path: string; stack: { handle: () => void }[] }
              }) => {
                return (
                  layer.route?.path === path &&
                  layer.route.stack.findIndex(
                    (sublayer) => sublayer.handle === routeHandler
                  ) !== -1
                )
              }
            )
            if (routeIndex !== -1) router.stack.splice(routeIndex, 1)
          },
        }
      },
    }

    return routeDescriptor
  }

  return Object.fromEntries(
    HTTP_METHODS.map((method) => [method, createBuilder(method)] as const)
  ) as RestApiBuilder
}
