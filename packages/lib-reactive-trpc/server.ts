import * as trpc from "@trpc/server"

import {
  Change,
  Reactor,
  TopicFactory,
  isSignal,
  isSynchronizableStore,
} from "@dassie/lib-reactive"

export const createRemoteReactiveRouter = <
  TExposedTopicsMap extends Record<string, TopicFactory>
>(
  exposedTopics: TExposedTopicsMap
) => {
  const validateTopicName = (topicName: unknown): keyof TExposedTopicsMap => {
    if (typeof topicName === "string" && topicName in exposedTopics) {
      return topicName as keyof TExposedTopicsMap
    }

    throw new Error("Invalid store name")
  }

  const router = trpc
    .router<Reactor>()
    .query("exposedTopics", {
      resolve: () => null as unknown as TExposedTopicsMap,
    })
    .query("getSignalState", {
      input: validateTopicName,
      resolve({ input: signalName, ctx: reactor }) {
        const topicFactory = exposedTopics[signalName]

        if (!topicFactory) {
          throw new Error("Invalid store name")
        }

        const signal = reactor.useContext(topicFactory)

        if (!isSignal(signal)) {
          throw new Error(`Topic is not a store`)
        }

        return {
          value: signal.read(),
        }
      },
    })
    .subscription("listenToTopic", {
      input: validateTopicName,
      resolve: <T extends keyof TExposedTopicsMap>({
        input: topicName,
        ctx: reactor,
      }: {
        input: T
        ctx: Reactor
      }) => {
        return new trpc.Subscription<unknown>((sendToClient) => {
          const topicFactory = exposedTopics[topicName]
          if (!topicFactory) {
            throw new Error("Invalid topic name")
          }
          const store = reactor.useContext(topicFactory)

          if (isSignal(store)) {
            sendToClient.data(store.read())
          }

          return store.on((value) => {
            sendToClient.data(value)
          })
        })
      },
    })
    .subscription("listenToChanges", {
      input: validateTopicName,
      resolve: <T extends keyof TExposedTopicsMap>({
        input: storeName,
        ctx: reactor,
      }: {
        input: T
        ctx: Reactor
      }) => {
        return new trpc.Subscription<
          | {
              type: "initial"
              data: unknown
            }
          | {
              type: "change"
              data: Change
            }
        >((sendToClient) => {
          const topicFactory = exposedTopics[storeName]
          if (!topicFactory) {
            throw new Error("Invalid topic name")
          }

          const store = reactor.useContext(topicFactory)

          if (!isSynchronizableStore(store)) {
            throw new Error("Target topic is not a synchronizable store")
          }

          sendToClient.data({
            type: "initial",
            data: store.read(),
          })

          const disposeSubscription = store.changes.on((change) => {
            sendToClient.data({
              type: "change",
              data: change,
            })
          })

          return () => {
            disposeSubscription()
            reactor.disposeContext(topicFactory)
          }
        })
      },
    })
    .flat()

  return router
}

export type RemoteReactiveRouter<TExposedTopicsMap extends Record<string, TopicFactory>> = ReturnType<typeof createRemoteReactiveRouter<TExposedTopicsMap>>