import { enableMapSet, produce } from "immer"

import { createStore } from "@dassie/lib-reactive"

import type { IlpAddress } from "../../ilp-connector/types/ilp-address"
import type { NodeId } from "../types/node-id"

enableMapSet()

export const UplinkAddressesStore = () =>
  createStore(new Map<NodeId, IlpAddress>()).actions({
    updateAddress: (peer: NodeId, newAddress: IlpAddress | undefined) =>
      produce((draft) => {
        if (!newAddress) {
          draft.delete(peer)
          return
        }
        draft.set(peer, newAddress)
      }),
  })
