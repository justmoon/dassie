import {
  type ReactNode,
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react"

import {
  Action,
  ActorContext,
  BoundAction,
  Factory,
  InferChanges,
  Store,
} from "@dassie/lib-reactive"
import { type Reactor, createReactor } from "@dassie/lib-reactive"

interface ReactActorContext {
  context: ActorContext
  wake: () => void
}

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

const noop = () => {
  // empty
}

const createReactActorContext = (reactor: Reactor) => {
  const reactActorContext: ReactActorContext = {
    context: new ActorContext("react", "react", reactor, () => {
      reactActorContext.wake()
    }),
    wake: noop,
  }
  reactActorContext.context.attachToParent(reactor)

  return reactActorContext
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

  const useSig = () => {
    const reactor = useReactor()

    const actorContext = useRef<ReactActorContext | undefined>()

    if (actorContext.current === undefined) {
      actorContext.current = createReactActorContext(reactor)
    }

    useEffect(() => {
      return () => {
        actorContext.current!.context.dispose().catch((error: unknown) => {
          console.error("error while disposing react actor context", { error })
        })
      }
    }, [reactor])

    return useSyncExternalStore(
      useCallback(
        (listener) => {
          const handleWake = () => {
            actorContext
              .current!.context.dispose()
              .catch((error: unknown) => {
                console.error("error while disposing react actor context", {
                  error,
                })
              })
              .finally(() => {
                actorContext
                  .current!.context.dispose()
                  .catch((error: unknown) => {
                    console.error("error while disposing react actor context", {
                      error,
                    })
                  })
                actorContext.current = createReactActorContext(reactor)
                actorContext.current.wake = handleWake
                listener()
              })
          }

          actorContext.current!.wake = handleWake

          return () => {
            actorContext.current!.wake = noop
          }
        },
        [reactor]
      ),
      () => actorContext.current!.context
    )
  }

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
    const sig = useSig()
    const store = useMemo(
      () => reactor.use(() => implementation(reactor)),
      [reactor, implementation]
    )

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
          }
        }
      },
    })

    sig.subscribe(store)
    return store.read()
  }

  return {
    Provider,
    useReactor,
    useSig,
    useRemoteSignal,
    useRemoteStore,
  }
}

export const { Provider, useReactor, useSig, useRemoteSignal, useRemoteStore } =
  createReactiveHooks()

export { createReactiveHooks }
