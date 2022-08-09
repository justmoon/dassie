import { useQuery } from "react-query"

import type { InferMessageType } from "@xen-ilp/lib-reactive"
import { createRemoteReactiveHooks } from "@xen-ilp/lib-reactive-trpc/client"

import type { ExposedStoresMap as NodeExposedStoresMap } from "../../runner/effects/debug-rpc-server"
import { client } from "./trpc"

export const getNodeRemoteStore = async <
  TStoreName extends keyof NodeExposedStoresMap
>(
  nodeId: string,
  storeName: TStoreName
) => {
  const { value } = await client.query("getNodeState", [nodeId, storeName])

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

const { useLiveRemoteStore } = createRemoteReactiveHooks(client)

export { useLiveRemoteStore }
