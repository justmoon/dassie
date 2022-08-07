import { useCallback, useSyncExternalStore } from "react"
import { useQuery } from "react-query"

import type { InferMessageType } from "@xen-ilp/lib-reactive"

import type { ExposedStoresMap } from "../../backend/rpc-routers/ui-rpc-router"
import type { ExposedStoresMap as NodeExposedStoresMap } from "../../runner/effects/debug-rpc-server"
import { client } from "./trpc"

export const getNodeRemoteStore = async <
  TStoreName extends keyof NodeExposedStoresMap
>(
  nodeId: string,
  storeName: TStoreName
) => {
  const { value } = await client.query("ui.getNodeState", [nodeId, storeName])

  return value as InferMessageType<NodeExposedStoresMap[TStoreName]>
}

export const useNodeRemoteStore = <
  TStoreName extends keyof NodeExposedStoresMap
>(
  nodeId: string,
  storeName: TStoreName
) => {
  return useQuery(["reactiveRemoteStore", nodeId, storeName], () =>
    getNodeRemoteStore(nodeId, storeName)
  )
}

interface ActiveSubscription<TStoreName extends keyof ExposedStoresMap> {
  listeners: Set<() => void>
  state: {
    data: InferMessageType<ExposedStoresMap[TStoreName]> | undefined
  }
  dispose?: () => void
}

export interface ActiveSubscriptionMap
  extends Map<
    keyof ExposedStoresMap,
    ActiveSubscription<keyof ExposedStoresMap>
  > {
  get<T extends keyof ExposedStoresMap>(
    key: T
  ): ActiveSubscription<T> | undefined
  set<T extends keyof ExposedStoresMap>(
    key: T,
    value: ActiveSubscription<T>
  ): this
}

const activeSubscriptions: ActiveSubscriptionMap = new Map()

const getActiveSubscriptionInfo = <TStoreName extends keyof ExposedStoresMap>(
  storeName: TStoreName
): ActiveSubscription<TStoreName> => {
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

const subscribe = <TStoreName extends keyof ExposedStoresMap>(
  storeName: TStoreName,
  callback: () => void
) => {
  const subscription = getActiveSubscriptionInfo(storeName)

  if (!subscription.dispose) {
    const dispose = client.subscription("ui.getLiveState", storeName, {
      onNext: (value) => {
        if (value.type !== "data") return

        const subscription = activeSubscriptions.get(storeName)

        if (!subscription) return

        subscription.state = {
          data: value.data as
            | InferMessageType<ExposedStoresMap[TStoreName]>
            | undefined,
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

export const useLiveRemoteStore = <TStoreName extends keyof ExposedStoresMap>(
  storeName: TStoreName
) => {
  return useSyncExternalStore(
    useCallback((listener) => subscribe(storeName, listener), [storeName]),
    () => getActiveSubscriptionInfo(storeName).state
  )
}
