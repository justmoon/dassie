import {
  type ReactNode,
  createContext,
  useContext,
  useMemo,
  useState,
} from "react"

import {
  Action,
  BoundAction,
  Factory,
  InferChanges,
  Store,
} from "@dassie/lib-reactive"
import { type Reactor, createReactor } from "@dassie/lib-reactive"

interface ProviderProperties {
  reactor?: Reactor | undefined
  children: ReactNode
}

export interface TrpcSubscriptionHook<TInput, TValue> {
  useSubscription: (
    input: TInput,
    options: {
      onData: (data: TValue) => void
    }
  ) => void
}

export interface TrpcStoreSubscriptionHook<TInitial, TChange> {
  useSubscription: (
    input: undefined,
    options: {
      onData: (
        data:
          | { type: "initial"; value: TInitial }
          | { type: "changes"; value: readonly TChange[] }
      ) => void
    }
  ) => void
}

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
  }: // eslint-disable-next-line unicorn/consistent-function-scoping
  TrpcSubscriptionHook<undefined, TValue>) => {
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
    TActions extends Record<string, Action<TState>>
  >(
    {
      useSubscription,
    }: TrpcStoreSubscriptionHook<TState, InferChanges<TActions>>,
    implementation: Factory<Store<TState, TActions>>
  ) => {
    const reactor = useReactor()
    const store = useMemo(
      () => reactor.use(() => implementation(reactor)),
      [reactor, implementation]
    )
    const [state, setState] = useState<{ state: TState }>({
      state: store.read(),
    })

    useSubscription(undefined, {
      onData: (data) => {
        if (data.type === "initial") {
          store.write(data.value)
        } else {
          for (const [actionName, parameters] of data.value) {
            const action = store[actionName] as
              | BoundAction<unknown, unknown[]>
              | undefined

            if (!action) {
              throw new Error(
                `Tried to synchronize action ${actionName} which does not exist in the local implementation`
              )
            }
            action(...parameters)
            setState({ state: store.read() })
          }
        }
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
