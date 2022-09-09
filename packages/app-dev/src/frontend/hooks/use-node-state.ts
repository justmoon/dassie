import { useEffect } from "react"
import { useQuery, useQueryClient } from "react-query"

import type { InferMessageType } from "@dassie/lib-reactive"

import type { ExposedStoresMap as NodeExposedStoresMap } from "../../runner/effects/debug-rpc-server"
import { nodeClients } from "../shared-effects/node-rpc-clients"

const fetchNodeRemoteStore = async <
  TStoreName extends keyof NodeExposedStoresMap
>(
  nodeId: string,
  storeName: TStoreName
) => {
  const [client, dispose] = nodeClients(nodeId)

  const result = await client.query("getSignalState", storeName)

  dispose()

  return result.value as InferMessageType<NodeExposedStoresMap[TStoreName]>
}

export const useNodeRemoteSignal = <
  TStoreName extends keyof NodeExposedStoresMap
>(
  nodeId: string,
  storeName: TStoreName
) => {
  const queryClient = useQueryClient()
  useEffect(() => {
    const [client, disposeClient] = nodeClients(nodeId)
    const disposeSubscription = client.subscription(
      "listenToTopic",
      storeName,
      {
        onNext() {
          void queryClient.invalidateQueries([
            "getNodeRemoteStore",
            nodeId,
            storeName,
          ])
        },
      }
    )

    return () => {
      disposeSubscription()
      disposeClient()
    }
  }, [queryClient, nodeId, storeName])

  return useQuery(["getNodeRemoteStore", nodeId, storeName], () =>
    fetchNodeRemoteStore(nodeId, storeName)
  )
}
