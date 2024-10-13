import { createReactiveHooks } from "./reactive"

export const {
  Provider,
  useReactor,
  useSignal,
  useRemoteSignal,
  useRemoteStore,
} = createReactiveHooks()

export { createReactiveHooks, type ProviderProperties } from "./reactive"

export {
  createRpcReact,
  type RpcHooks,
  type DeriveHooksRouter,
} from "./rpc/hooks"

export type { UseQueryHook, UseQueryHookOptions } from "./rpc/types/use-query"

export type {
  RpcContext,
  RpcContextValue,
  RpcProviderProperties,
} from "./rpc/context"

export type {
  UseMutationHook,
  UseMutationHookOptions,
} from "./rpc/types/use-mutation"

export type {
  UseSubscriptionHook,
  UseSubscriptionHookOptions,
} from "./rpc/types/use-subscription"

export type {
  UseWebSocketClientHook,
  UseWebSocketClientOptions,
} from "./rpc/use-websocket-client"
