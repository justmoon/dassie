import type {
  MutateOptions,
  QueryClient,
  UseMutationOptions,
  UseMutationResult,
} from "@tanstack/react-query"
import type { Merge } from "type-fest"

import type { HooksRouteSettings } from "./hooks-route"

export type UseMutationHook<TRouteSettings extends HooksRouteSettings> = (
  options?: UseMutationHookOptions<TRouteSettings>,
) => Merge<
  UseMutationResult<TRouteSettings["output"], Error, TRouteSettings["input"]>,
  {
    mutate: (
      ...parameters: TRouteSettings["input"] extends undefined ?
        [
          input?: TRouteSettings["input"],
          options?: MutateOptions<TRouteSettings["output"]> | undefined,
        ]
      : [
          input: TRouteSettings["input"],
          options?: MutateOptions<TRouteSettings["output"]> | undefined,
        ]
    ) => void
  }
>

export type UseMutationHookOptions<
  TRouteSettings extends HooksRouteSettings = HooksRouteSettings,
> = Merge<
  Omit<
    UseMutationOptions<TRouteSettings["output"]>,
    "mutationKey" | "mutationFn"
  >,
  {
    queryClient?: QueryClient | undefined
  }
>
