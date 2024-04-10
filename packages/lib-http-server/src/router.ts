import type { Promisable, SetNonNullable, Simplify } from "type-fest"
import type { AnyZodObject, infer as InferZodType } from "zod"

import { AnyOerType, Infer as InferOerType } from "@dassie/lib-oer"
import type { LifecycleContext } from "@dassie/lib-reactive"
import { Failure, isFailure } from "@dassie/lib-type-utils"

import { handleError } from "./handle-error"
import {
  parseBodyBuffer,
  parseBodyOer,
  parseBodyUint8Array,
  parseBodyUtf8,
  parseBodyZod,
  parseJson,
} from "./middlewares/parse-body"
import { parseQueryParameters } from "./middlewares/query-parameters"
import { PREFIX_WILDCARD, SEGMENT_WILDCARD, Trie } from "./trie/trie"
import { type RequestContext } from "./types/context"
import { HttpFailure } from "./types/http-failure"
import type { HttpResponse } from "./types/http-response"
import { RouteParameters } from "./types/route-parameters"
import type { WebSocketRequestContext } from "./types/websocket"
import { isDynamicPath } from "./utils/is-dynamic-path"
import { normalizePath } from "./utils/normalize-path"
import { parseParameters } from "./utils/parse-parameters"

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
  path: <const TPath extends string>(
    path: TPath,
  ) => ApiRouteBuilder<
    TPath extends `${string}:${string}`
      ? TParameters & {
          parameters: Simplify<RouteParameters<TPath>>
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

interface RouteBuilderState<TInitialContext extends object> {
  readonly path: string | undefined
  readonly method: HttpMethod | undefined
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  readonly middlewares: Array<Middleware<any, object>>
  readonly userHandler: ApiHandler<TInitialContext> | undefined
}

const initialRouteBuilderState: RouteBuilderState<never> = {
  path: undefined,
  method: undefined,
  userHandler: undefined,
  middlewares: [],
}

type RouteHandler<TInitialContext extends object> = (
  context: RequestContext & TInitialContext,
) => Promise<HttpFailure | HttpResponse>

function createRouteHandler<TInitialContext extends object>(
  state: SetNonNullable<RouteBuilderState<TInitialContext>, "userHandler">,
): RouteHandler<TInitialContext> {
  return async (context) => {
    for (const middleware of state.middlewares) {
      const result = await middleware(context)
      if (isFailure(result)) return result
      if (result) Object.assign(context, result)
    }

    return await state.userHandler(context)
  }
}

type StaticRouteKey<
  TMethod extends string = string,
  TPath extends string = string,
> = `${TMethod} ${TPath}`

export function createRouter<TInitialContext extends object = object>() {
  const staticRoutes = new Map<StaticRouteKey, RouteHandler<TInitialContext>>()
  const dynamicRoutes = new Map<
    HttpMethod,
    Trie<RouteHandler<TInitialContext>>
  >()

  function createBuilder(state: RouteBuilderState<TInitialContext>) {
    const routeDescriptor: ApiRouteBuilder<TInitialContext> = {
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

          // Set the user handler
          userHandler: handler,
        }

        const normalizedPath = normalizePath(path)
        const isDynamicRoute = isDynamicPath(normalizedPath)

        if (!isDynamicRoute) {
          const routeHandler = createRouteHandler<TInitialContext>(finalState)

          const staticRouteKey: StaticRouteKey = `${method} ${path}`
          if (staticRoutes.has(staticRouteKey)) {
            throw new Error(`Route already exists for ${method} ${path}`)
          }

          staticRoutes.set(staticRouteKey, routeHandler)

          context.lifecycle.onCleanup(() => {
            staticRoutes.delete(staticRouteKey)
          })
          return
        }

        const prefixWildcardIndex = normalizedPath.indexOf("*")

        if (
          prefixWildcardIndex !== -1 &&
          prefixWildcardIndex !== normalizedPath.length - 1
        ) {
          throw new Error("Prefix wildcard must be at the end of the path")
        }

        const parameterMap: string[] = []

        // Remove the parameter names from the path since the trie doesn't
        // need them. We keep track of the parameter names so that we can
        // later map them back.
        const anonymizedPath = normalizedPath.map((segment) => {
          if (segment.startsWith(":")) {
            let wildcardType = SEGMENT_WILDCARD
            if (segment.endsWith("*")) {
              wildcardType = PREFIX_WILDCARD
              segment = segment.slice(0, -1)
            }
            const parameterName = segment.slice(1)

            if (!parameterName) {
              throw new Error("Parameter name must not be empty")
            }

            parameterMap.push(parameterName)

            return wildcardType
          }

          if (segment.includes("*") || segment.includes("?")) {
            throw new Error("URL must not include * or ? characters")
          }

          return segment
        })

        const parameterParserMiddleware = (context: RequestContext) => {
          return {
            parameters: parseParameters(
              context.url.pathname,
              anonymizedPath,
              parameterMap,
            ),
          }
        }

        finalState.middlewares.unshift(parameterParserMiddleware)

        let trie = dynamicRoutes.get(method)

        if (!trie) {
          trie = new Trie()
          dynamicRoutes.set(method, trie)
        }

        const result = trie.insert(
          anonymizedPath,
          createRouteHandler<TInitialContext>(finalState),
        )

        if (isFailure(result)) {
          throw new Error("Route already exists for path")
        }

        context.lifecycle.onCleanup(() => {
          trie.remove(anonymizedPath)

          if (trie.root.size === 0) {
            dynamicRoutes.delete(method)
          }
        })
      },
    }

    return routeDescriptor
  }

  function match(method: string, path: string) {
    const staticRoute = staticRoutes.get(`${method.toLowerCase()} ${path}`)
    if (staticRoute) return staticRoute

    const trie = dynamicRoutes.get(method.toLowerCase() as HttpMethod)

    if (!trie) return undefined

    const normalizedPath = normalizePath(path)

    const dynamicRoute = trie.get(normalizedPath)

    if (!dynamicRoute) return undefined

    return dynamicRoute
  }

  return {
    ...createBuilder(
      initialRouteBuilderState as RouteBuilderState<TInitialContext>,
    ),
    match,
    handle: async (
      context: RequestContext & TInitialContext,
    ): Promise<Response> => {
      try {
        const handler = match(context.request.method, context.url.pathname)

        if (handler) {
          const result = await handler(context)
          return result.asResponse(context)
        }

        return new Response("Not Found", { status: 404 })
      } catch (error: unknown) {
        return handleError(context, error)
      }
    },
  }
}

export const createWebSocketRouter = () => {
  return createRouter<WebSocketRequestContext>()
}
