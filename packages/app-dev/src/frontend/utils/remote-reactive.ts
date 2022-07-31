import { useQuery } from "react-query"

import type { StoreFactory } from "@xen-ilp/lib-reactive"

import type { ExposedStoresMap } from "../../runner/effects/debug-rpc-server"
import { client } from "./trpc"

export type InferStoreModelType<TStore> = TStore extends StoreFactory<
  infer M,
  never
>
  ? M
  : never

export const getRemoteStore = async <TStoreName extends keyof ExposedStoresMap>(
  nodeId: string,
  storeName: TStoreName
) => {
  const { value } = await client.query("ui.getState", [nodeId, storeName])

  return value as InferStoreModelType<ExposedStoresMap[TStoreName]>
}

export const useRemoteStore = <TStoreName extends keyof ExposedStoresMap>(
  nodeId: string,
  storeName: TStoreName
) => {
  return useQuery(["reactiveRemoteStore", nodeId, storeName], () =>
    getRemoteStore(nodeId, storeName)
  )
}
