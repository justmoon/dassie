import type { Request, Response } from "express"
import { Router } from "express"
import { Promisable, Simplify } from "type-fest"
import type { AnyZodObject, infer as InferZodType } from "zod"

import { IncomingMessage } from "node:http"

import { AnyOerType, Infer as InferOerType } from "@dassie/lib-oer"
import type { LifecycleContext } from "@dassie/lib-reactive"
import { Failure, isFailure } from "@dassie/lib-type-utils"

import { cors } from "./cors"
import { HttpRequestHandler, createHandler } from "./handler"
import {
  parseBodyBuffer,
  parseBodyOer,
  parseBodyUint8Array,
  parseBodyUtf8,
  parseBodyZod,
  parseJson,
} from "./parse-body"
import { parseQueryParameters } from "./query-parameters"
import { HttpFailure } from "./types/http-failure"
import { RouteParameters } from "./types/route-parameters"

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

export const BODY_PARSERS = {
  json: parseJson,
  uint8Array: parseBodyUint8Array,
  utf8: parseBodyUtf8,
  buffer: parseBodyBuffer,
} as const

export type BodyParser = keyof typeof BODY_PARSERS

export type RestApiBuilder = {
  [method in HttpMethod]: () => ApiRouteBuilder
}

export type ApiRouteParameters = object

export type ApiHandler<TParameters extends ApiRouteParameters> =
  HttpRequestHandler<Simplify<TParameters>>

export type InferBodyType<TBodyParser extends BodyParser> = Exclude<
  Awaited<ReturnType<(typeof BODY_PARSERS)[TBodyParser]>>,
  Failure
>["body"]

export type Middleware<TInput extends object, TOutput extends object> = (
  request: Request & TInput,
  response: Response,
) => Promisable<void | TOutput | HttpFailure>

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export type AnyMiddleware = Middleware<any, object>

export type ApplyMiddleware<
  TParameters extends ApiRouteParameters,
  TMiddleware extends AnyMiddleware,
> = TParameters & Exclude<Awaited<ReturnType<TMiddleware>>, Failure>

export type ApiRouteBuilder<
  TParameters extends ApiRouteParameters = ApiRouteParameters,
> = {
  /**
   * Provide a JSON validator for the request body.
   */
  path: <TPath extends string>(
    path: TPath,
  ) => ApiRouteBuilder<
    // Only add a parameters type to the object if there are actual parameters
    object extends RouteParameters<TPath>
      ? TParameters & {
          params: RouteParameters<TPath>
        }
      : TParameters
  >

  /**
   * Set HTTP method.
   */
  method: (method: HttpMethod) => ApiRouteBuilder<TParameters>

  /**
   * Retrieve the request body and put it into the body property on the request object.
   */
  bodyParser: <TBodyParser extends BodyParser>(
    type: TBodyParser,
  ) => ApiRouteBuilder<
    TParameters & {
      body: InferBodyType<TBodyParser>
    }
  >

  /**
   * Provide a JSON validator for the request body.
   */
  bodySchemaZod: <TBodySchema extends AnyZodObject>(
    schema: TBodySchema,
  ) => ApiRouteBuilder<
    TParameters & {
      body: InferZodType<TBodySchema>
    }
  >

  /**
   * Provide an OER deserializer for the request body.
   */
  bodySchemaOer: <TBodySchema extends AnyOerType>(
    schema: TBodySchema,
  ) => ApiRouteBuilder<
    TParameters & {
      body: InferOerType<TBodySchema>
    }
  >

  /**
   * Provide a validator for the query string record.
   */
  querySchema: <TQuerySchema extends AnyZodObject>(
    schema: TQuerySchema,
  ) => ApiRouteBuilder<
    TParameters & {
      query: InferZodType<TQuerySchema>
    }
  >

  use: <TMiddleware extends Middleware<TParameters, object>>(
    middleware: TMiddleware,
  ) => ApiRouteBuilder<ApplyMiddleware<TParameters, TMiddleware>>

  /**
   * Provide a function which will be run against each request before the
   * handler and which may return a failure to be returned to the client.
   */
  assert: (
    assertion: (request: IncomingMessage) => HttpFailure | void,
  ) => ApiRouteBuilder<TParameters>

  /**
   * Enable CORS for this route.
   */
  cors: () => ApiRouteBuilder<TParameters>

  /**
   * Provide the handler for the API route.
   */
  handler: <THandler extends ApiHandler<TParameters>>(
    lifecycle: LifecycleContext,
    handler: THandler,
  ) => void
} & {
  [K in HttpMethod]: () => ApiRouteBuilder<TParameters>
}

interface RouteBuilderState {
  readonly path: string | undefined
  readonly method: HttpMethod | undefined
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  readonly middlewares: Array<Middleware<any, object>>
  readonly userHandler: ApiHandler<ApiRouteParameters> | undefined
}

const initialRouteBuilderState: RouteBuilderState = {
  path: undefined,
  method: undefined,
  userHandler: undefined,
  middlewares: [],
}

const createRouteHandler =
  (state: RouteBuilderState) =>
  async (request: Request, response: Response) => {
    if (!state.userHandler) {
      throw new Error("No handler provided for API route")
    }

    for (const middleware of state.middlewares) {
      const result = await middleware(request, response)
      if (isFailure(result)) return result
      if (result) Object.assign(request, result)
    }

    return state.userHandler(request, response)
  }

export const createRouter = () => {
  const router = Router()

  const createBuilder = (state: RouteBuilderState) => {
    const routeDescriptor: ApiRouteBuilder<ApiRouteParameters> = {
      path: (value) => {
        return createBuilder({
          ...state,
          path: value,
        })
      },
      method: (method) => {
        return createBuilder({
          ...state,
          method,
        })
      },
      ...(Object.fromEntries(
        HTTP_METHODS.map(
          (method) =>
            [
              method,
              () =>
                createBuilder({
                  ...state,
                  method,
                }),
            ] as const,
        ),
      ) as RestApiBuilder),
      bodyParser: (type: BodyParser) => {
        return createBuilder({
          ...state,
          middlewares: [...state.middlewares, BODY_PARSERS[type]],
        })
      },
      bodySchemaZod: (schema) => {
        return createBuilder({
          ...state,
          middlewares: [...state.middlewares, parseBodyZod(schema)],
        })
      },
      bodySchemaOer: (schema) => {
        return createBuilder({
          ...state,
          middlewares: [...state.middlewares, parseBodyOer(schema)],
        })
      },
      querySchema: (schema) => {
        return createBuilder({
          ...state,
          middlewares: [...state.middlewares, parseQueryParameters(schema)],
        })
      },
      cors: () => {
        return createBuilder({
          ...state,
          middlewares: [...state.middlewares, cors],
        })
      },
      use: (middleware) => {
        return createBuilder({
          ...state,
          middlewares: [...state.middlewares, middleware],
        })
      },
      assert: (assertion) => {
        return createBuilder({
          ...state,
          middlewares: [...state.middlewares, assertion],
        })
      },
      handler: (context, handler) => {
        const { path, method } = state
        if (!path) {
          throw new Error("No path provided for route")
        }
        if (!method) {
          throw new Error("No method provided for route")
        }

        const finalState = {
          ...state,
          userHandler: handler,
        }

        const routeHandler = createRouteHandler(finalState)

        router[method](path, createHandler(routeHandler))

        context.lifecycle.onCleanup(() => {
          const routeIndex = router.stack.findIndex(
            (layer: {
              route?: {
                path: string
                stack: { handle: HttpRequestHandler }[]
              }
            }) => {
              return (
                // path is always a string here because we check for it above
                // and while it can be changed later, it can only be changed
                // to another string
                layer.route?.path === path &&
                layer.route.stack.findIndex(
                  (sublayer) => sublayer.handle === routeHandler,
                ) !== -1
              )
            },
          )
          if (routeIndex !== -1) router.stack.splice(routeIndex, 1)
        })
      },
    }

    return routeDescriptor
  }

  return {
    ...createBuilder(initialRouteBuilderState),
    middleware: router,
  }
}
