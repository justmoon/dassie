import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useSyncExternalStore,
} from "react"

import {
  Effect,
  Reactor,
  StoreFactory,
  TopicFactory,
  createReactor,
} from "@dassie/lib-reactive"

export const createReactiveHooks = () => {
  const ReactorContext = createContext<Reactor>(createReactor(() => undefined))

  const useReactor = () => useContext(ReactorContext)

  const useSigEffect = <TResult>(effect: Effect<undefined, TResult>) => {
    const reactor = useReactor()
    useEffect(() => {
      const { dispose } = reactor.run(effect)

      return () => {
        dispose().catch((error: unknown) => {
          console.error("error while cleaning up reactor effect", {
            effect: effect.name,
            error,
          })
        })
      }
    }, [reactor, effect])
  }

  const useTopic = <TMessage>(
    topicFactory: TopicFactory<TMessage>,
    callback: (message: TMessage) => void
  ) => {
    const reactor = useContext(ReactorContext)

    useEffect(() => {
      const disposeSubscription = reactor.useContext(topicFactory).on(callback)
      return () => {
        disposeSubscription()
        reactor.disposeContext(topicFactory)
      }
    }, [topicFactory, callback])
  }

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
    useSigEffect,
    useReactor,
    useTopic,
    useStore,
  }
}

const { Provider, useSigEffect, useReactor, useTopic, useStore } =
  createReactiveHooks()

export { Provider, useSigEffect, useReactor, useTopic, useStore }
