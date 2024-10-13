import type { QueryClient } from "@tanstack/react-query"

const SUBSCRIPTION_STORE_KEY = ["__dassie-rpc-subscription-store"]

const DISPOSAL_DELAY = 1000

type SubscriptionStoreEntry = {
  referenceCount: number
  disposer: () => void
  disposalTimeout: ReturnType<typeof setTimeout> | undefined
}

interface SubscriptionStore {
  increment(subscriptionKey: string, effect: () => () => void): void
  decrement(subscriptionKey: string): void
}

export function getSubscriptionStore(queryClient: QueryClient) {
  // Never garbage collect the subscription store itself
  queryClient.setQueryDefaults(SUBSCRIPTION_STORE_KEY, {
    gcTime: 0,
  })

  let subscriptionStore: SubscriptionStore | undefined =
    queryClient.getQueryData(SUBSCRIPTION_STORE_KEY)

  if (!subscriptionStore) {
    subscriptionStore = createSubscriptionStore()

    queryClient.setQueryData(SUBSCRIPTION_STORE_KEY, subscriptionStore)
  }

  return subscriptionStore
}

function createSubscriptionStore(): SubscriptionStore {
  const activeSubscriptions = new Map<string, SubscriptionStoreEntry>()

  return {
    increment(subscriptionKey, effect) {
      const entry = activeSubscriptions.get(subscriptionKey)

      if (entry) {
        entry.referenceCount++

        if (entry.disposalTimeout) {
          clearTimeout(entry.disposalTimeout)
          entry.disposalTimeout = undefined
        }
      } else {
        activeSubscriptions.set(subscriptionKey, {
          referenceCount: 1,
          disposer: effect(),
          disposalTimeout: undefined,
        })
      }
    },
    decrement(subscriptionKey) {
      const entry = activeSubscriptions.get(subscriptionKey)

      if (!entry || entry.referenceCount <= 0) {
        throw new Error("Reference count went negative, this is a bug")
      }

      entry.referenceCount--

      if (entry.referenceCount === 0 && !entry.disposalTimeout) {
        entry.disposalTimeout = setTimeout(() => {
          entry.disposer()
          activeSubscriptions.delete(subscriptionKey)
        }, DISPOSAL_DELAY)
      }
    },
  }
}
