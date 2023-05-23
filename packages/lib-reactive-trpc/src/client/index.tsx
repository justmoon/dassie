import type { inferRouterProxyClient } from "@trpc/client"
import type { AnyRouter } from "@trpc/server"
import {
  type ReactNode,
  createContext,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useSyncExternalStore,
} from "react"

import {
  type Actor,
  ActorContext,
  type BoundAction,
  type Change,
  type InferMessageType,
  createActor,
  isStore,
} from "@dassie/lib-reactive"
import { type Reactor, createReactor } from "@dassie/lib-reactive"

import type { ExposedTopicsMap, RemoteReactiveRouter } from "../server"

interface ReactActorContext {
  context: ActorContext
  wake: () => void
}

export type ReactiveTrpcClient<TExposedTopicsMap extends ExposedTopicsMap> =
  inferRouterProxyClient<RemoteReactiveRouter<TExposedTopicsMap>>

interface ProviderProperties {
  reactor?: Reactor | undefined
  trpcClient: inferRouterProxyClient<AnyRouter>
  children: ReactNode
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

const createReactiveHooks = <TExposedTopicsMap extends ExposedTopicsMap>() => {
  const ReactorContext = createContext<Reactor | undefined>(undefined)

  const TrpcClient = (): ReactiveTrpcClient<TExposedTopicsMap> => {
    throw new Error(
      "Tried to instantiate a TRPC client which should have been injected automatically when the reactor was given to the context provider"
    )
  }

  const buildReactor = (
    providedReactor: Reactor | undefined,
    trpcClient: ReactiveTrpcClient<TExposedTopicsMap>
  ) => {
    const reactor = providedReactor ?? createReactor()
    reactor.set(TrpcClient, trpcClient)
    return reactor
  }

  const Provider = ({ reactor, trpcClient, children }: ProviderProperties) => {
    const ContextProvider = ReactorContext.Provider
    return (
      <ContextProvider
        value={buildReactor(
          reactor,
          trpcClient as ReactiveTrpcClient<TExposedTopicsMap>
        )}
      >
        {children}
      </ContextProvider>
    )
  }

  const useReactor = () => {
    const context = useContext(ReactorContext)
    if (!context) throw new Error("Must use hooks inside of Provider")
    return context
  }

  const useTrpcClient = () => {
    const context = useContext(ReactorContext)
    if (!context) throw new Error("Must use hooks inside of Provider")
    return context.peek(TrpcClient)
  }

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

  const createRemoteTopic = <
    TTopicName extends string & keyof TExposedTopicsMap
  >(
    topicName: TTopicName
  ): Actor<InferMessageType<TExposedTopicsMap[TTopicName]> | undefined> => {
    const service = createActor<
      InferMessageType<TExposedTopicsMap[TTopicName]> | undefined
    >((sig) => {
      const client = sig.use(TrpcClient)

      // eslint-disable-next-line @typescript-eslint/no-unsafe-argument
      const subscription = client.listenToTopic.subscribe(topicName, {
        onData: (event: unknown) => {
          service.write(
            event as Awaited<InferMessageType<TExposedTopicsMap[TTopicName]>>
          )
        },
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
      } as any)
      sig.onCleanup(() => subscription.unsubscribe())

      return undefined
    })

    return service
  }

  const createRemoteSignal = <
    TSignalName extends string & keyof TExposedTopicsMap,
    TInitialValue
  >(
    storeName: TSignalName,
    initialValue: TInitialValue
  ): Actor<
    InferMessageType<TExposedTopicsMap[TSignalName]> | TInitialValue
  > => {
    const service = createActor<
      InferMessageType<TExposedTopicsMap[TSignalName]> | TInitialValue
    >((sig) => {
      const trpcClient = sig.use(TrpcClient)

      // eslint-disable-next-line @typescript-eslint/no-unsafe-argument
      const subscription = trpcClient.listenToTopic.subscribe(storeName, {
        onData: (event: unknown) => {
          service.write(
            event as Awaited<InferMessageType<TExposedTopicsMap[TSignalName]>>
          )
        },
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
      } as any)
      sig.onCleanup(() => subscription.unsubscribe())

      return initialValue
    })

    return service
  }

  const createRemoteStore = <
    TStoreName extends string & keyof TExposedTopicsMap
  >(
    storeName: TStoreName,
    storeImplementation: TExposedTopicsMap[TStoreName]
  ): Actor<InferMessageType<TExposedTopicsMap[TStoreName]>> => {
    const service = createActor<
      InferMessageType<TExposedTopicsMap[TStoreName]>
    >((sig) => {
      const client = sig.use(TrpcClient)

      const localStore = sig.use(storeImplementation)

      if (!isStore(localStore)) {
        throw new Error("Store is not synchronizable")
      }

      localStore.on(sig, (newValue) => {
        service.write(
          newValue as Awaited<InferMessageType<TExposedTopicsMap[TStoreName]>>
        )
      })

      // eslint-disable-next-line @typescript-eslint/no-unsafe-argument
      const subscription = client.listenToChanges.subscribe(storeName, {
        onData: (
          event:
            | { type: "initial"; data: unknown }
            | { type: "change"; data: Change }
        ) => {
          if (event.type === "initial") {
            localStore.write(event.data)
          } else {
            const action = localStore[event.data[0]] as
              | BoundAction<unknown, unknown[]>
              | undefined

            if (!action) {
              throw new Error(
                `Tried to synchronize action ${event.data[0]} which does not exist in the local implementation`
              )
            }
            action(...event.data[1])
          }
        },
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
      } as any)
      sig.onCleanup(() => subscription.unsubscribe())

      return localStore.read() as InferMessageType<
        TExposedTopicsMap[TStoreName]
      >
    })

    return service
  }

  return {
    Provider,
    useReactor,
    useTrpcClient,
    useSig,
    createRemoteTopic,
    createRemoteSignal,
    createRemoteStore,
  }
}

export { createReactiveHooks }
