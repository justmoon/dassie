import type { Promisable, SetNonNullable, Simplify } from "type-fest"

import type { ScopeContext } from "@dassie/lib-reactive"
import { Failure, isFailure } from "@dassie/lib-type-utils"

import { handleError } from "./handle-error"
import { PREFIX_WILDCARD, SEGMENT_WILDCARD, Trie } from "./trie/trie"
import type { BaseRequestContext } from "./types/context"
import { HttpFailure } from "./types/http-failure"
import type { HttpResponse } from "./types/http-response"
import { RouteParameters } from "./types/route-parameters"
import type { WebSocketRequestContext } from "./types/websocket"
import { isDynamicPath } from "./utils/is-dynamic-path"
import { normalizePath } from "./utils/normalize-path"
import { parseParameters } from "./utils/parse-parameters"
import { WebSocketResponse } from "./websocket/websocket-response"

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
  TContext extends BaseRequestContext = BaseRequestContext,
> = (context: TContext) => Promisable<HttpFailure | HttpResponse>

export type MethodShortcuts = {
  [method in HttpMethod]: () => AnyRouteBuilder
}

export type UserRouteHandler<TParameters extends {}> = (
  context: TParameters,
) => Promisable<HttpFailure | HttpResponse>

export type Middleware<TInput extends {}, TOutput extends {}> = (
  context: BaseRequestContext & TInput,
) => Promisable<void | TOutput | HttpFailure>

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export type AnyMiddleware = Middleware<any, {}>

export type ApplyMiddleware<
  TParameters extends {},
  TMiddleware extends AnyMiddleware,
> = TParameters & Exclude<Awaited<ReturnType<TMiddleware>>, void | Failure>

export type RouteBuilder<TParameters extends { route: RouteContext }> = {
  /**
   * Provide a JSON validator for the request body.
   */
  path: <const TPath extends string>(
    path: TPath,
  ) => RouteBuilder<
    Simplify<
      Omit<TParameters, "parameters" | "route"> & {
        parameters: TPath extends `${string}:${string}` ?
          Simplify<RouteParameters<TPath>>
        : {}

        route: TParameters["route"] extends RouteContext<infer TMethod> ?
          RouteContext<TMethod, TPath>
        : never
      }
    >
  >

  /**
   * Set HTTP method.
   */
  method: <const TMethod extends HttpMethod>(
    method: TMethod,
  ) => RouteBuilder<
    Simplify<
      Omit<TParameters, "route"> & {
        route: TParameters["route"] extends (
          RouteContext<HttpMethod, infer TPath>
        ) ?
          RouteContext<TMethod, TPath>
        : never
      }
    >
  >

  use: <TMiddleware extends Middleware<TParameters, object>>(
    middleware: TMiddleware,
  ) => RouteBuilder<ApplyMiddleware<TParameters, TMiddleware>>

  /**
   * Provide a function which will be run against each request before the
   * handler and which may return a failure to be returned to the client.
   */
  assert: (
    assertion: (
      context: BaseRequestContext & TParameters,
    ) => HttpFailure | void,
  ) => RouteBuilder<TParameters>

  /**
   * Provide the handler for the API route.
   */
  handler: <
    THandler extends UserRouteHandler<BaseRequestContext & TParameters>,
  >(
    scope: ScopeContext,
    handler: THandler,
  ) => void
} & {
  [K in HttpMethod]: () => RouteBuilder<
    TParameters & {
      route: TParameters["route"] extends (
        RouteContext<HttpMethod, infer TPath>
      ) ?
        RouteContext<K, TPath>
      : never
    }
  >
}

interface RouteBuilderState {
  readonly path: string | undefined
  readonly method: HttpMethod | undefined
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  readonly middlewares: Array<Middleware<any, object>>
  readonly userHandler: UserRouteHandler<Record<string, unknown>> | undefined
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type AnyRouteBuilder = RouteBuilder<any>

const initialRouteBuilderState: RouteBuilderState = {
  path: undefined,
  method: undefined,
  userHandler: undefined,
  middlewares: [],
}

type InternalRouteHandler<TContext extends object> = (
  context: TContext,
) => Promise<HttpFailure | HttpResponse>

function createRouteHandler<TContext extends object>(
  state: SetNonNullable<RouteBuilderState, "path" | "method" | "userHandler">,
): InternalRouteHandler<TContext> {
  return async (context) => {
    const routeContext = Object.assign(context, {
      route: {
        method: state.method,
        path: state.path,
      },
    })

    for (const middleware of state.middlewares) {
      const result = await middleware(context)
      if (isFailure(result)) return result
      if (result) Object.assign(context, result)
    }

    return await state.userHandler(routeContext)
  }
}

type StaticRouteKey<
  TMethod extends string = string,
  TPath extends string = string,
> = `${TMethod} ${TPath}`

export interface RouteContext<
  TMethod extends HttpMethod = HttpMethod,
  TPath extends string = string,
> {
  route: {
    method: TMethod
    path: TPath
  }
}

export function createRouteMatcher<TInitialContext extends {} = {}>() {
  const staticRoutes = new Map<
    StaticRouteKey,
    InternalRouteHandler<TInitialContext>
  >()
  const dynamicRoutes = new Map<
    HttpMethod,
    Trie<InternalRouteHandler<TInitialContext>>
  >()

  function createBuilder(state: RouteBuilderState): AnyRouteBuilder {
    return {
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
      ) as MethodShortcuts),
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
          path,
          method,

          // Remove duplicate middlewares
          middlewares: [...new Set(middlewares)],

          // Set the user handler
          userHandler: handler as UserRouteHandler<Record<string, unknown>>,
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

          context.scope.onCleanup(() => {
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
          } else if (segment === "*") {
            // anonymous wildcard path
            parameterMap.push("*")

            return PREFIX_WILDCARD
          }

          if (segment.includes("*") || segment.includes("?")) {
            throw new Error("URL must not include * or ? characters")
          }

          return segment
        })

        const parameterParserMiddleware = (context: BaseRequestContext) => {
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

        context.scope.onCleanup(() => {
          trie.remove(anonymizedPath)

          if (trie.root.size === 0) {
            dynamicRoutes.delete(method)
          }
        })
      },
    } as AnyRouteBuilder
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
    ...(createBuilder(initialRouteBuilderState) as RouteBuilder<
      TInitialContext & { route: RouteContext }
    >),
    match,
  }
}

export function createRouter() {
  const matcher = createRouteMatcher()
  return {
    ...matcher,

    handle: async (context: BaseRequestContext): Promise<Response> => {
      try {
        const handler = matcher.match(
          context.request.method,
          context.url.pathname,
        )

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

export function createWebSocketRouter() {
  const matcher = createRouteMatcher<WebSocketRequestContext>()
  return {
    ...matcher,

    handle: async (
      context: BaseRequestContext & WebSocketRequestContext,
    ): Promise<Response | void> => {
      try {
        const handler = matcher.match(
          context.request.method,
          context.url.pathname,
        )

        if (handler) {
          const result = await handler(context)

          if (!(result instanceof WebSocketResponse)) {
            return result.asResponse(context)
          }
        }
      } catch (error: unknown) {
        return handleError(context, error)
      }
    },
  }
}
