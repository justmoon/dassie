import type { TRPCClient } from "@trpc/client"
import type { DefaultErrorShape } from "@trpc/server"
import type { Procedure } from "@trpc/server/dist/declarations/src/internals/procedure"
import type {
  ProcedureRecord,
  Router,
} from "@trpc/server/dist/declarations/src/router"
import { useCallback, useSyncExternalStore } from "react"

import type {
  InferMessageType,
  Reactor,
  TopicFactory,
} from "@xen-ilp/lib-reactive"

export interface RemoteReactiveHooks<
  TExposedTopicsMap extends Record<string, TopicFactory>
> {
  useLiveRemoteStore: <TStoreName extends string & keyof TExposedTopicsMap>(
    storeName: TStoreName
  ) => { data: InferMessageType<TExposedTopicsMap[TStoreName]> | undefined }
}

export const createRemoteReactiveHooks = <
  TExposedTopicsMap extends Record<string, TopicFactory>
>(
  client: TRPCClient<
    Router<
      Reactor,
      Reactor,
      Record<string, unknown>,
      {
        exposedTopics: Procedure<
          Reactor,
          Reactor,
          unknown,
          undefined,
          undefined,
          TExposedTopicsMap,
          unknown,
          TExposedTopicsMap
        >
        getStoreState: Procedure<
          Reactor,
          Reactor,
          unknown,
          string,
          string,
          unknown,
          unknown
        >
      } & ProcedureRecord,
      ProcedureRecord,
      ProcedureRecord,
      DefaultErrorShape
    >
  >
): RemoteReactiveHooks<TExposedTopicsMap> => {
  interface ActiveSubscription<TStoreName extends keyof TExposedTopicsMap> {
    listeners: Set<() => void>
    state: {
      data: InferMessageType<TExposedTopicsMap[TStoreName]> | undefined
    }
    dispose?: () => void
  }

  interface ActiveSubscriptionMap
    extends Map<
      keyof TExposedTopicsMap,
      ActiveSubscription<keyof TExposedTopicsMap>
    > {
    get<T extends keyof TExposedTopicsMap>(
      key: T
    ): ActiveSubscription<T> | undefined
    set<T extends keyof TExposedTopicsMap>(
      key: T,
      value: ActiveSubscription<T>
    ): this
  }

  const activeSubscriptions: ActiveSubscriptionMap = new Map()

  const getActiveSubscriptionInfo = <
    TTopicName extends keyof TExposedTopicsMap
  >(
    storeName: TTopicName
  ): ActiveSubscription<TTopicName> => {
    let subscription = activeSubscriptions.get(storeName)

    if (!subscription) {
      subscription = {
        listeners: new Set(),
        state: { data: undefined },
      }

      activeSubscriptions.set(storeName, subscription)
    }

    return subscription
  }

  const subscribe = <TStoreName extends string & keyof TExposedTopicsMap>(
    storeName: TStoreName,
    callback: () => void
  ) => {
    const subscription = getActiveSubscriptionInfo(storeName)

    if (!subscription.dispose) {
      client
        .query(`getStoreState`, storeName)
        .then((result: unknown) => {
          const subscription = activeSubscriptions.get(storeName)

          if (!subscription) return

          subscription.state = {
            data: (
              result as {
                value: InferMessageType<TExposedTopicsMap[TStoreName]>
              }
            ).value,
          }

          for (const listener of subscription.listeners) listener()
        })
        .catch((error) => {
          console.error(error)
        })
      const dispose = client.subscription(`listenToTopic`, storeName, {
        onNext: (value) => {
          if (value.type !== "data") return

          const subscription = activeSubscriptions.get(storeName)

          if (!subscription) return

          subscription.state = {
            data: value.data as InferMessageType<TExposedTopicsMap[TStoreName]>,
          }

          for (const listener of subscription.listeners) {
            listener()
          }
        },
      })

      subscription.dispose = dispose
    }

    subscription.listeners.add(callback)

    return () => {
      const subscription = getActiveSubscriptionInfo(storeName)

      subscription.listeners.delete(callback)

      if (subscription.listeners.size === 0) {
        subscription.dispose?.()
        delete subscription.dispose
      }
    }
  }

  const useLiveRemoteStore = <
    TStoreName extends string & keyof TExposedTopicsMap
  >(
    storeName: TStoreName
  ) => {
    return useSyncExternalStore(
      useCallback((listener) => subscribe(storeName, listener), [storeName]),
      () => {
        return getActiveSubscriptionInfo(storeName).state
      }
    )
  }

  return {
    useLiveRemoteStore,
  }
}
