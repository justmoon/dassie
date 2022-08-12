import * as trpc from "@trpc/server"

import {
  Change,
  Reactor,
  TopicFactory,
  isStore,
  isSynchronizableStore,
} from "@dassie/lib-reactive"

export const createRemoteReactiveRouter = <
  TExposedTopicsMap extends Record<string, TopicFactory>
>(
  exposedTopics: TExposedTopicsMap
) => {
  const validateStoreName = (storeName: unknown): keyof TExposedTopicsMap => {
    if (typeof storeName === "string" && storeName in exposedTopics) {
      return storeName as keyof TExposedTopicsMap
    }

    throw new Error("Invalid store name")
  }

  const router = trpc
    .router<Reactor>()
    .query("exposedTopics", {
      resolve: () => null as unknown as TExposedTopicsMap,
    })
    .query("getStoreState", {
      input: validateStoreName,
      resolve({ input: storeName, ctx: reactor }) {
        const topicFactory = exposedTopics[storeName]

        if (!topicFactory) {
          throw new Error("Invalid store name")
        }

        const store = reactor.useContext(topicFactory)

        if (!isStore(store)) {
          throw new Error(`Topic is not a store`)
        }

        return {
          value: store.read(),
        }
      },
    })
    .subscription("listenToTopic", {
      input: validateStoreName,
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

          if (isStore(store)) {
            sendToClient.data(store.read())
          }

          return store.on((value) => {
            sendToClient.data(value)
          })
        })
      },
    })
    .subscription("listenToChanges", {
      input: validateStoreName,
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