import { createReactiveHooks } from "@dassie/lib-reactive-trpc/client"

const hooks = createReactiveHooks<Record<string, never>>()

export const { Provider, useSig } = hooks
