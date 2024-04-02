import type { Promisable, SetNonNullable, Simplify } from "type-fest"
import type { AnyZodObject, infer as InferZodType } from "zod"

import { AnyOerType, Infer as InferOerType } from "@dassie/lib-oer"
import type { LifecycleContext } from "@dassie/lib-reactive"
import { Failure, isFailure } from "@dassie/lib-type-utils"

import { type RequestContext, createContext } from "./context"
import { handleError } from "./handle-error"
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
import type { HttpResponse } from "./types/http-response"
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

export type HttpRequestHandler<
  TAdditionalRequestFields extends object = object,
> = (
  context: RequestContext<TAdditionalRequestFields>,
) => Promisable<HttpFailure | HttpResponse>

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
  context: RequestContext<TInput>,
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
    assertion: (context: RequestContext) => HttpFailure | void,
  ) => ApiRouteBuilder<TParameters>

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

type RouteHandler = (
  context: RequestContext,
) => Promise<HttpFailure | HttpResponse>

const createRouteHandler =
  (state: SetNonNullable<RouteBuilderState, "userHandler">): RouteHandler =>
  async (context) => {
    for (const middleware of state.middlewares) {
      const result = await middleware(context)
      if (isFailure(result)) return result
      if (result) Object.assign(context, result)
    }

    return await state.userHandler(context)
  }

type RouteKey<
  TMethod extends string = string,
  TPath extends string = string,
> = `${TMethod} ${TPath}`

export const createRouter = () => {
  const routes = new Map<RouteKey, RouteHandler>()

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
        const { path, method, middlewares } = state
        if (!path) {
          throw new Error("No path provided for route")
        }
        if (!method) {
          throw new Error("No method provided for route")
        }

        const finalState = {
          ...state,

          // Remove duplicate middlewares
          middlewares: [...new Set(middlewares)],

          userHandler: handler,
        }

        const routeHandler = createRouteHandler(finalState)

        if (routes.has(`${method} ${path}`)) {
          throw new Error(`Route already exists for ${method} ${path}`)
        }

        routes.set(`${method} ${path}`, routeHandler)

        context.lifecycle.onCleanup(() => {
          routes.delete(`${method} ${path}`)
        })
      },
    }

    return routeDescriptor
  }

  function match(method: string, path: string) {
    return routes.get(`${method.toLowerCase()} ${path}`)
  }

  return {
    ...createBuilder(initialRouteBuilderState),
    match,
    handle: async (request: Request) => {
      const context = createContext(request)

      try {
        const handler =
          match(request.method, context.url.pathname) ??
          match(request.method, "*")

        if (!handler) {
          return new Response("Not Found", { status: 404 })
        }

        const result = await handler(context)
        return result.asResponse(context)
      } catch (error: unknown) {
        return handleError(context, error)
      }
    },
  }
}
