import type { TRPCClient } from "@trpc/client"

import {
  BoundAction,
  EffectContext,
  InferMessageType,
  TopicFactory,
  Value,
  ValueFactory,
  createValue,
  isSynchronizableStore,
} from "@dassie/lib-reactive"

import type { RemoteReactiveRouter } from "./server"

export interface ReactiveTrpcConnection<
  TExposedTopicsMap extends Record<string, TopicFactory>
> {
  client: ReactiveTrpcClient<TExposedTopicsMap>
}

export type ReactiveTrpcClient<
  TExposedTopicsMap extends Record<string, TopicFactory>
> = TRPCClient<RemoteReactiveRouter<TExposedTopicsMap>>

export const createTrpcConnectionValue = <
  TClient extends TRPCClient<RemoteReactiveRouter<Record<string, TopicFactory>>>
>(
  connect: (sig: EffectContext) => TClient
) => {
  return createValue((sig) => {
    const trpcClient = sig.run(connect)

    return { client: trpcClient } as const
  })
}

export const createRemoteTopic = <
  TExposedTopicsMap extends Record<string, TopicFactory>,
  TTopicName extends string & keyof TExposedTopicsMap
>(
  connectionFactory: ValueFactory<ReactiveTrpcConnection<TExposedTopicsMap>>,
  topicName: TTopicName
): Value<InferMessageType<TExposedTopicsMap[TTopicName]> | undefined> => {
  return createValue((sig, value) => {
    const { client } = sig.get(connectionFactory)

    sig.onCleanup(
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      client.subscription("listenToTopic", topicName as any, {
        onNext: (event) => {
          if (event.type !== "data") return

          value.emit(
            () => event.data as InferMessageType<TExposedTopicsMap[TTopicName]>
          )
        },
      })
    )

    return undefined
  })
}

export const createRemoteStore = <
  TExposedTopicsMap extends Record<string, TopicFactory>,
  TStoreName extends string & keyof TExposedTopicsMap,
  TInitialValue
>(
  connectionFactory: ValueFactory<ReactiveTrpcConnection<TExposedTopicsMap>>,
  storeName: TStoreName,
  initialValue: TInitialValue
): Value<InferMessageType<TExposedTopicsMap[TStoreName]> | TInitialValue> => {
  return createValue((sig, value) => {
    const { client } = sig.get(connectionFactory)

    sig.onCleanup(
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      client.subscription("listenToTopic", storeName as any, {
        onNext: (event) => {
          if (event.type !== "data") return

          value.emit(
            () => event.data as InferMessageType<TExposedTopicsMap[TStoreName]>
          )
        },
      })
    )

    return initialValue
  })
}

export const createRemoteSynchronizedStore = <
  TExposedTopicsMap extends Record<string, TopicFactory>,
  TStoreName extends string & keyof TExposedTopicsMap
>(
  connectionFactory: ValueFactory<ReactiveTrpcConnection<TExposedTopicsMap>>,
  storeName: TStoreName,
  storeImplementation: TExposedTopicsMap[TStoreName]
): Value<InferMessageType<TExposedTopicsMap[TStoreName]>> => {
  return createValue((sig, value) => {
    const { client } = sig.get(connectionFactory)

    const localStore = sig.run(storeImplementation)

    if (!isSynchronizableStore(localStore)) {
      throw new Error("Store is not synchronizable")
    }

    sig.onCleanup(
      localStore.on((newValue) => {
        value.emit(
          () => newValue as InferMessageType<TExposedTopicsMap[TStoreName]>
        )
      })
    )

    sig.onCleanup(
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      client.subscription("listenToChanges", storeName as any, {
        onNext: (event) => {
          if (event.type !== "data") return
          if (event.data.type === "initial") {
            localStore.emit(() => event.data.data)
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
}
