import { initTRPC } from '@trpc/server';

import {
  Reactor,
  TopicFactory,
  isSignal,
  Change,
  isStore,
} from "@dassie/lib-reactive"
import { observable } from '@trpc/server/observable';

// eslint-disable-next-line @typescript-eslint/consistent-type-definitions
export type ReactiveContext = {reactor: Reactor}

const trpc = initTRPC.context<ReactiveContext>().create()

export const createRemoteReactiveRouter = <
  TExposedTopicsMap extends Record<string, TopicFactory>,
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
    .router({
      exposedTopics: trpc.procedure.query(() => (null as unknown as TExposedTopicsMap)),
      getSignalState: trpc.procedure.input(validateTopicName).query(({ input: signalName, ctx: { reactor }}) => {
        const topicFactory = exposedTopics[signalName]

        if (!topicFactory) {
          throw new Error("Invalid store name")
        }

        const signal = reactor.use(topicFactory)

        if (!isSignal(signal)) {
          throw new Error(`Topic is not a store`)
        }

        return {
          value: signal.read()
        }
      }),
      listenToTopic: trpc.procedure.input(validateTopicName).subscription(({ input: topicName, ctx: { reactor } }) => {
        return observable<unknown>((emit) => {
          const topicFactory = exposedTopics[topicName]
          if (!topicFactory) {
            throw new Error("Invalid topic name")
          }
          const store = reactor.use(topicFactory)

          if (isSignal(store)) {
            emit.next(store.read())
          }

          return store.on((value) => {
            emit.next(value)
          })
        })
      }),
      listenToChanges: trpc.procedure.input(validateTopicName).subscription(({ input: storeName, ctx: { reactor } }) => {
        return observable<
          | {
              type: "initial"
              data: unknown
            }
          | {
              type: "change"
              data: Change
            }
        >((emit) => {
          const topicFactory = exposedTopics[storeName]
          if (!topicFactory) {
            throw new Error("Invalid topic name")
          }

          const store = reactor.use(topicFactory)

          if (!isStore(store)) {
            throw new Error("Target is not a store")
          }

          emit.next({
            type: "initial",
            data: store.read(),
          })

          const disposeSubscription = store.changes.on((change) => {
            emit.next({
              type: "change",
              data: change,
            })
          })

          return () => {
            disposeSubscription()
            reactor.delete(topicFactory)
          }
        })
      })
    })

  return router
}

export type RemoteReactiveRouter<TExposedTopicsMap extends Record<string, TopicFactory>> = ReturnType<typeof createRemoteReactiveRouter<TExposedTopicsMap>>