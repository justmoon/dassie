import type {
  QueryClient,
  UseQueryOptions,
  UseQueryResult,
} from "@tanstack/react-query"
import type { Merge } from "type-fest"

import type { HooksRouteSettings } from "./hooks-route"

export type UseQueryHook<TRouteSettings extends HooksRouteSettings> = (
  ...parameters: TRouteSettings["input"] extends undefined ?
    [
      input?: TRouteSettings["input"],
      options?: UseQueryHookOptions<TRouteSettings> | undefined,
    ]
  : [
      input: TRouteSettings["input"],
      options?: UseQueryHookOptions<TRouteSettings> | undefined,
    ]
) => UseQueryResult<TRouteSettings["output"]>

export type UseQueryHookOptions<
  TRouteSettings extends HooksRouteSettings = HooksRouteSettings,
> = Merge<
  Omit<UseQueryOptions<TRouteSettings["output"]>, "queryKey" | "queryFn">,
  {
    queryClient?: QueryClient | undefined
  }
>
