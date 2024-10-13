import type { QueryClient } from "@tanstack/react-query"

import type { Subscription } from "@dassie/lib-rpc/client"

import type { HooksRouteSettings } from "./hooks-route"

export type UseSubscriptionHook<TRouteSettings extends HooksRouteSettings> = (
  ...parameters: TRouteSettings["input"] extends undefined ?
    [
      input?: TRouteSettings["input"],
      options?:
        | UseSubscriptionHookOptions<
            TRouteSettings["output"] extends Subscription<infer T> ? T : never,
            Error
          >
        | undefined,
    ]
  : [
      input: TRouteSettings["input"],
      options?:
        | UseSubscriptionHookOptions<
            TRouteSettings["output"] extends Subscription<infer T> ? T : never,
            Error
          >
        | undefined,
    ]
) => void

export type UseSubscriptionHookOptions<TData = unknown, TError = unknown> = {
  onData: (data: TData) => void
  onError?: (error: TError) => void
  queryClient?: QueryClient | undefined
}
