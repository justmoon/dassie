import * as trpc from "@trpc/server"

import { Reactor, TopicFactory, isStore } from "@xen-ilp/lib-reactive"

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

          return store.on((value) => {
            sendToClient.data(value)
          })
        })
      },
    })
    .flat()

  return router
}
