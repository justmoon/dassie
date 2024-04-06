import type { RouteType } from "../server/router/route"

export const VALID_ROUTE_TYPES = ["query", "mutation", "subscription"] as const

export const VALID_ROUTE_VERBS = ["query", "mutate", "subscribe"] as const

export const ROUTE_HANDLER_TO_TYPE_MAP: Record<string, RouteType> = {
  query: "query",
  mutate: "mutation",
  subscribe: "subscription",
} as const
