import type { AnyRoute } from "../../server/router/route"
import type { AnyRouter } from "../../server/router/router"

interface ClientRouteSettings {
  type: "query" | "mutation" | "subscription"
  input: unknown
  output: unknown
}

export type ClientRoute<TRouteSettings extends ClientRouteSettings> =
  TRouteSettings["type"] extends "query" ? QueryRoute<TRouteSettings>
  : TRouteSettings["type"] extends "mutation" ? MutationRoute<TRouteSettings>
  : TRouteSettings["type"] extends "subscription" ?
    SubscriptionRoute<TRouteSettings>
  : never

export interface QueryRoute<TRouteSettings extends ClientRouteSettings> {
  query(
    ...inputParameters: undefined extends TRouteSettings["input"] ?
      [input?: TRouteSettings["input"]]
    : [input: TRouteSettings["input"]]
  ): Promise<TRouteSettings["output"]>
  type: "query"
  path: string[]
}

export interface MutationRoute<TRouteSettings extends ClientRouteSettings> {
  mutate(
    ...inputParameters: undefined extends TRouteSettings["input"] ?
      [input?: TRouteSettings["input"]]
    : [input: TRouteSettings["input"]]
  ): Promise<TRouteSettings["output"]>
  type: "mutation"
  path: string[]
}

export interface SubscriptionRoute<TRouteSettings extends ClientRouteSettings> {
  subscribe(
    ...inputParameters: undefined extends TRouteSettings["input"] ?
      [input?: TRouteSettings["input"]]
    : [input: TRouteSettings["input"]]
  ): Promise<TRouteSettings["output"]>
  type: "subscription"
  path: string[]
}

export type DeriveClientRouter<TRouter extends AnyRouter> = {
  [K in keyof TRouter["routes"]]: DeriveClientRoute<TRouter["routes"][K]>
}
export type DeriveClientRoute<TRoute> =
  TRoute extends AnyRoute ?
    ClientRoute<{
      type: TRoute["type"]
      input: Parameters<TRoute>[0]["input"]
      output: Awaited<ReturnType<TRoute>>
    }>
  : TRoute extends AnyRouter ? DeriveClientRouter<TRoute>
  : never
