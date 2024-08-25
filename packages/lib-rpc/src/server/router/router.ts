import type { UnionToIntersection } from "type-fest"

import { RpcFailure } from "../../common/rpc-failure"
import { RpcSuccess } from "../../common/rpc-success"
import { type AnyRoute, type RouteType } from "./route"

export type RouteDefinition = Record<string, AnyRoute | AnyRouter>

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export type AnyRouter = Router<any>

export type Router<TRoutes extends RouteDefinition = RouteDefinition> = {
  routes: TRoutes
  call(
    parameters: RouterCallParameters<DeriveContextFromRoutes<TRoutes>>,
  ): Promise<RpcSuccess | RpcFailure>
}

export interface RouterCallParameters<TContext extends object> {
  path: string[]
  type: RouteType
  context: TContext
  input: unknown
}

// Derive the context type from the route definition
// The context is the intersection of all context types of all routes
// For a route, the context is the route's context
// For a router, the context is determined recursively
export type DeriveContextFromRoutes<TRoutes extends RouteDefinition> =
  UnionToIntersection<
    {
      [K in keyof TRoutes]: TRoutes[K] extends AnyRoute ?
        Parameters<TRoutes[K]>[0]["context"]
      : TRoutes[K] extends Router<infer TSubroutes> ?
        DeriveContextFromRoutes<TSubroutes>
      : never
    }[keyof TRoutes]
  > & {}

export type DeriveContextFromRouter<TRouter extends AnyRouter> = Parameters<
  TRouter["call"]
>[0]["context"]

const DISALLOWED_KEYS = new Set([
  "call",
  "apply",
  "then",
  "catch",
  "key",
  "path",
  "prototype",
  "constructor",
  "routes",
  "context",
  "input",
  "output",
  "type",
  "inputSchema",
  "middlewares",
  "query",
  "mutation",
  "subscription",
  "use",
  "mutate",
  "subscribe",
])

export function createRouter<TRoutes extends RouteDefinition>(
  routes: TRoutes,
): Router<TRoutes> {
  for (const key of Object.keys(routes)) {
    if (DISALLOWED_KEYS.has(key)) {
      throw new Error(`Invalid route definition: key "${key}" is not allowed`)
    }
  }

  return {
    routes,
    async call(parameters) {
      const pathSegments = parameters.path

      let currentRoutes: RouteDefinition = routes
      while (pathSegments.length > 0) {
        if (!Object.hasOwn(currentRoutes, pathSegments[0]!)) {
          break
        }

        const route = currentRoutes[pathSegments[0]!]!

        // Current segment is a router, decend into it
        if (typeof route !== "function") {
          // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
          currentRoutes = route.routes
          pathSegments.shift()
          continue
        }

        // Current segment is a route, call it

        // We should be at the last segment
        if (pathSegments.length !== 1) break

        const { inputSchema, type } = route

        if (type !== parameters.type) {
          return new RpcFailure(
            `Invalid route type: tried to access "${parameters.path.join("/")}" as "${parameters.type}" but it is a "${type}"`,
          )
        }

        const input = inputSchema.safeParse(parameters.input)

        if (!input.success) {
          return new RpcFailure(
            `Invalid input: ${input.error.errors.map((error) => error.message).join(", ")}`,
          )
        }

        return new RpcSuccess(
          await route({
            context: parameters.context,
            input: input.data as unknown,
          }),
        )
      }

      return new RpcFailure(
        `RPC method not found: ${parameters.path.join("/")}`,
      )
    },
  }
}
