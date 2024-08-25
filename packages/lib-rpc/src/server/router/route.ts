import type { Merge, Promisable, Simplify } from "type-fest"
import { type infer as ZodInfer, type ZodTypeAny, z } from "zod"

import { isFailure } from "@dassie/lib-type-utils"

import type { VALID_ROUTE_TYPES } from "../../common/route-type"
import type { RpcFailure } from "../../common/rpc-failure"
import type { Subscription } from "../../common/subscription"

export type AnyRoute = Route<{
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  _context: any
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  _input: any
  _output: unknown
  type: RouteType
  inputSchema: ZodTypeAny
  // eslint-disable-next-line @typescript-eslint/no-unsafe-function-type
  middlewares: Function[]
}>

export type RouteType = (typeof VALID_ROUTE_TYPES)[number]

export type Route<TRouteSettings extends RouteSettings> = {
  (
    parameters: Simplify<RouteParameters<TRouteSettings>>,
  ): Promise<TRouteSettings["_output"]>
  type: TRouteSettings["type"]
  inputSchema: ZodTypeAny
}

interface RouteSettings {
  _context: {}
  _input: unknown
  _output: unknown
  type: RouteType
  inputSchema: ZodTypeAny
  // eslint-disable-next-line @typescript-eslint/no-unsafe-function-type
  middlewares: Function[]
}

export interface RouteBuilder<
  TRouteSettings extends RouteSettings = RouteSettings,
> {
  input<TInput extends ZodTypeAny>(
    schema: TInput,
  ): RouteBuilder<Merge<TRouteSettings, { _input: ZodInfer<TInput> }>>

  context<TContext extends object>(): RouteBuilder<
    Merge<TRouteSettings, { _context: TRouteSettings["_context"] & TContext }>
  >

  use(
    middleware: (
      parameters: RouteParameters<TRouteSettings>,
    ) => Promisable<void | RpcFailure>,
  ): RouteBuilder<TRouteSettings>

  query<
    THandler extends (parameters: {
      input: TRouteSettings["_input"]
      context: TRouteSettings["_context"]
    }) => Promisable<unknown>,
  >(
    handler: THandler,
  ): Route<
    Merge<TRouteSettings, { type: "query"; _output: ReturnType<THandler> }>
  >

  mutation<
    THandler extends (parameters: {
      input: TRouteSettings["_input"]
      context: TRouteSettings["_context"]
    }) => Promisable<unknown>,
  >(
    handler: THandler,
  ): Route<
    Merge<TRouteSettings, { type: "mutation"; _output: ReturnType<THandler> }>
  >

  subscription<
    THandler extends (parameters: {
      input: TRouteSettings["_input"]
      context: TRouteSettings["_context"]
    }) => Promisable<Subscription<unknown>>,
  >(
    handler: THandler,
  ): Route<
    Merge<
      TRouteSettings,
      { type: "subscription"; _output: ReturnType<THandler> }
    >
  >
}

interface RouteParameters<TRouteSettings extends RouteSettings> {
  input: TRouteSettings["_input"]
  context: TRouteSettings["_context"]
}

type InitialRouteSettings = Merge<RouteSettings, { _input: undefined }>

const INITIAL_ROUTE_SETTINGS: InitialRouteSettings = {
  _context: undefined as unknown as object,
  _input: undefined,
  _output: undefined,
  type: "query",
  inputSchema: z.unknown(),
  middlewares: [],
}

export function createRoute(): RouteBuilder<InitialRouteSettings> {
  function createBuilder<TRouteSettings extends RouteSettings>(
    settings: TRouteSettings,
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
  ): RouteBuilder<any> {
    function buildHandler<
      TType extends RouteType,
      THandler extends (parameters: {
        input: TRouteSettings["_input"]
        context: TRouteSettings["_context"]
      }) => Promisable<unknown>,
    >(type: TType, handler: THandler) {
      return Object.assign(
        async (
          parameters: RouteParameters<TRouteSettings>,
        ): Promise<Awaited<ReturnType<THandler>>> => {
          for (const middleware of settings.middlewares) {
            const result = await (
              middleware as (
                parameters: RouteParameters<TRouteSettings>,
              ) => Promisable<void | RpcFailure>
            )(parameters)

            if (isFailure(result)) {
              return result as Awaited<ReturnType<THandler>>
            }
          }
          return (await handler(parameters)) as Awaited<ReturnType<THandler>>
        },
        {
          type,
          inputSchema: settings.inputSchema,
        },
      )
    }

    return {
      input(input: ZodTypeAny) {
        return createBuilder({
          ...settings,
          inputSchema: input,
        })
      },
      context() {
        return createBuilder(settings)
      },

      use(middleware) {
        return createBuilder({
          ...settings,
          middlewares: [...settings.middlewares, middleware],
        })
      },
      query: (handler) => buildHandler("query", handler),
      mutation: (handler) => buildHandler("mutation", handler),
      subscription: (handler) => buildHandler("subscription", handler),
    }
  }

  return createBuilder(
    INITIAL_ROUTE_SETTINGS,
  ) as RouteBuilder<InitialRouteSettings>
}
