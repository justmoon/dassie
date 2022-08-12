import {
  createContext,
  useCallback,
  useContext,
  useSyncExternalStore,
} from "react"

import { Reactor, StoreFactory, createReactor } from "@dassie/lib-reactive"

export const createReactiveHooks = () => {
  const ReactorContext = createContext<Reactor>(createReactor(() => undefined))

  const useReactor = () => useContext(ReactorContext)

  const useStore = <TState>(
    storeFactory: StoreFactory<TState>
  ): TState | undefined => {
    const reactor = useContext(ReactorContext)

    return useSyncExternalStore(
      useCallback(
        (listener) => {
          const store = reactor.useContext(storeFactory)
          const disposeSubscription = store.on(listener)
          return () => {
            disposeSubscription()
            reactor.disposeContext(storeFactory)
          }
        },
        [storeFactory]
      ),
      () => {
        return reactor.peekContext(storeFactory)?.read()
      }
    )
  }

  return {
    Provider: ReactorContext.Provider,
    useReactor,
    useStore,
  }
}

const { Provider, useReactor, useStore } = createReactiveHooks()

export { Provider, useReactor, useStore }
