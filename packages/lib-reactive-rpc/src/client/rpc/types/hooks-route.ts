import type { UseMutationHook } from "./use-mutation"
import type { UseQueryHook } from "./use-query"
import type { UseSubscriptionHook } from "./use-subscription"

export interface HooksRouteSettings {
  type: "query" | "mutation" | "subscription"
  input: unknown
  output: unknown
}

export type HooksRoute<TRouteSettings extends HooksRouteSettings> =
  TRouteSettings["type"] extends "query" ?
    {
      useQuery: UseQueryHook<TRouteSettings>
    }
  : TRouteSettings["type"] extends "mutation" ?
    {
      useMutation: UseMutationHook<TRouteSettings>
    }
  : TRouteSettings["type"] extends "subscription" ?
    {
      useSubscription: UseSubscriptionHook<TRouteSettings>
    }
  : never
