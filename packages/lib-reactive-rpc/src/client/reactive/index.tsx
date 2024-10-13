import { type ReactNode, createContext, useContext, useState } from "react"

import type {
  Action,
  BoundAction,
  Factory,
  FactoryOrInstance,
  InferChanges,
  ReadonlySignal,
  Store,
} from "@dassie/lib-reactive"
import {
  FactoryNameSymbol,
  type Reactor,
  createReactor,
  createScope,
  defaultSelector,
} from "@dassie/lib-reactive"
import type { Subscription } from "@dassie/lib-rpc/client"

import type { UseSubscriptionHook } from "../rpc/types/use-subscription"
import { useSyncExternalStoreWithSelector } from "./hooks/use-sync-external-store-with-selector"

export interface ProviderProperties {
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

export const createReactiveHooks = () => {
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

  const useSignal = <TValue, TSelection = TValue>(
    signalFactory: FactoryOrInstance<ReadonlySignal<TValue>>,
    selector: (value: TValue) => TSelection = defaultSelector as unknown as (
      value: TValue,
    ) => TSelection,
  ): TSelection => {
    const reactor = useReactor()

    const signal =
      typeof signalFactory === "function" ?
        reactor.use(signalFactory)
      : signalFactory

    return useSyncExternalStoreWithSelector(
      (onStoreChange) => {
        const scope = createScope(`useSignal(${signal[FactoryNameSymbol]})`)

        signal.values.on(scope, onStoreChange)

        return () => {
          scope.dispose().catch((error: unknown) => {
            console.error("error disposing signal", { error })
          })
        }
      },
      () => signal.read(),
      undefined,
      selector,
    )
  }

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
    useSignal,
    useRemoteSignal,
    useRemoteStore,
  }
}
