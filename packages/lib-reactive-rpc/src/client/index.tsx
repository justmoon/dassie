import { type ReactNode, createContext, useContext, useState } from "react"

import type {
  Action,
  BoundAction,
  Factory,
  InferChanges,
  Store,
} from "@dassie/lib-reactive"
import { type Reactor, createReactor } from "@dassie/lib-reactive"
import type { UseSubscriptionHook } from "@dassie/lib-rpc-react"
import type { Subscription } from "@dassie/lib-rpc/client"

interface ProviderProperties {
  reactor?: Reactor | undefined
  children: ReactNode
}

export interface RpcSubscriptionHook<TInput, TValue> {
  useSubscription: UseSubscriptionHook<{
    type: "subscription"
    input: TInput
    output: Subscription<TValue>
  }>
}

export type StoreSubscriptionData<TInitial, TChange> =
  | { type: "initial"; value: TInitial }
  | { type: "changes"; value: TChange }

const createReactiveHooks = () => {
  const ReactorContext = createContext<Reactor>(createReactor())

  const Provider = ({ reactor, children }: ProviderProperties) => {
    const ContextProvider = ReactorContext.Provider
    return (
      <ContextProvider value={reactor ?? createReactor()}>
        {children}
      </ContextProvider>
    )
  }

  const useReactor = () => useContext(ReactorContext)

  const useRemoteSignal = <TValue,>({
    useSubscription,
    // eslint-disable-next-line unicorn/consistent-function-scoping
  }: RpcSubscriptionHook<undefined, TValue>) => {
    const [state, setState] = useState<TValue | undefined>(undefined)
    useSubscription(undefined, {
      onData: (data) => {
        setState(data)
      },
    })
    return state
  }

  const useRemoteStore = <
    TState,
    TActions extends Record<string, Action<TState>>,
  >(
    {
      useSubscription,
    }: RpcSubscriptionHook<
      undefined,
      StoreSubscriptionData<TState, InferChanges<TActions>>
    >,
    implementation: Factory<Store<TState, TActions>>,
  ) => {
    const reactor = useReactor()
    const store = reactor.use(implementation)
    const [state, setState] = useState<{ state: TState }>({
      state: store.read(),
    })

    useSubscription(undefined, {
      onData: (data) => {
        if (data.type === "initial") {
          store.write(data.value)
        } else {
          const [actionName, parameters] = data.value
          const action = store.act[actionName] as
            | BoundAction<unknown, unknown[]>
            | undefined

          if (!action) {
            throw new Error(
              `Tried to synchronize action ${actionName} which does not exist in the local implementation`,
            )
          }
          action(...parameters)
        }
        setState({ state: store.read() })
      },
    })

    return state.state
  }

  return {
    Provider,
    useReactor,
    useRemoteSignal,
    useRemoteStore,
  }
}

export const { Provider, useReactor, useRemoteSignal, useRemoteStore } =
  createReactiveHooks()

export { createReactiveHooks }
