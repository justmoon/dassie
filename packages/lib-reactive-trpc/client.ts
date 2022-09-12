import type { TRPCClient } from "@trpc/client"

import {
  BoundAction,
  EffectContext,
  InferMessageType,
  Service,
  ServiceFactory,
  TopicFactory,
  createService,
  isStore,
} from "@dassie/lib-reactive"

import type { RemoteReactiveRouter } from "./server"

export type ReactiveTrpcClient<
  TExposedTopicsMap extends Record<string, TopicFactory>
> = TRPCClient<RemoteReactiveRouter<TExposedTopicsMap>>

export const createTrpcConnection = <
  TClient extends TRPCClient<RemoteReactiveRouter<Record<string, TopicFactory>>>
>(
  connect: (sig: EffectContext) => TClient
) => {
  return createService(connect)
}

export const createRemoteTopic = <
  TExposedTopicsMap extends Record<string, TopicFactory>,
  TTopicName extends string & keyof TExposedTopicsMap
>(
  connectionFactory: ServiceFactory<
    ReactiveTrpcClient<TExposedTopicsMap>,
    unknown
  >,
  topicName: TTopicName
): Service<
  InferMessageType<TExposedTopicsMap[TTopicName]> | undefined,
  unknown
> => {
  const service = createService<
    InferMessageType<TExposedTopicsMap[TTopicName]> | undefined,
    unknown
  >((sig) => {
    const client = sig.get(connectionFactory)

    if (!client) return

    sig.onCleanup(
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      client.subscription("listenToTopic", topicName as any, {
        onNext: (event) => {
          if (event.type !== "data") return

          service.write(
            event.data as InferMessageType<TExposedTopicsMap[TTopicName]>
          )
        },
      })
    )

    return undefined
  })

  return service
}

export const createRemoteSignal = <
  TExposedTopicsMap extends Record<string, TopicFactory>,
  TSignalName extends string & keyof TExposedTopicsMap,
  TInitialValue
>(
  connectionFactory: ServiceFactory<
    ReactiveTrpcClient<TExposedTopicsMap>,
    unknown
  >,
  storeName: TSignalName,
  initialValue: TInitialValue
): Service<
  InferMessageType<TExposedTopicsMap[TSignalName]> | TInitialValue,
  unknown
> => {
  const service = createService<
    InferMessageType<TExposedTopicsMap[TSignalName]> | TInitialValue,
    unknown
  >((sig) => {
    const client = sig.get(connectionFactory)

    if (!client) return initialValue

    sig.onCleanup(
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      client.subscription("listenToTopic", storeName as any, {
        onNext: (event) => {
          if (event.type !== "data") return

          service.write(
            event.data as InferMessageType<TExposedTopicsMap[TSignalName]>
          )
        },
      })
    )

    return initialValue
  })

  return service
}

export const createRemoteSynchronizedStore = <
  TExposedTopicsMap extends Record<string, TopicFactory>,
  TStoreName extends string & keyof TExposedTopicsMap
>(
  connectionFactory: ServiceFactory<
    ReactiveTrpcClient<TExposedTopicsMap>,
    unknown
  >,
  storeName: TStoreName,
  storeImplementation: TExposedTopicsMap[TStoreName]
): Service<InferMessageType<TExposedTopicsMap[TStoreName]>, unknown> => {
  const service = createService<
    InferMessageType<TExposedTopicsMap[TStoreName]>,
    unknown
  >((sig) => {
    const client = sig.get(connectionFactory)

    const localStore = sig.run(storeImplementation)

    if (!isStore(localStore)) {
      throw new Error("Store is not synchronizable")
    }

    if (!client)
      return localStore.read() as InferMessageType<
        TExposedTopicsMap[TStoreName]
      >

    sig.onCleanup(
      localStore.on((newValue) => {
        service.write(
          newValue as InferMessageType<TExposedTopicsMap[TStoreName]>
        )
      })
    )

    sig.onCleanup(
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      client.subscription("listenToChanges", storeName as any, {
        onNext: (event) => {
          if (event.type !== "data") return
          if (event.data.type === "initial") {
            localStore.write(event.data.data)
          } else {
            const action = localStore[event.data.data[0]] as
              | BoundAction<unknown, unknown[]>
              | undefined

            if (!action) {
              throw new Error(
                `Tried to synchronize action ${event.data.data[0]} which does not exist in the local implmentation`
              )
            }
            action(...event.data.data[1])
          }
        },
      })
    )

    return localStore.read() as InferMessageType<TExposedTopicsMap[TStoreName]>
  })

  return service
}
