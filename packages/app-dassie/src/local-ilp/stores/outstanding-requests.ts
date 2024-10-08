import { castImmutable, enableMapSet, produce } from "immer"

import type { IlpResponsePacket } from "@dassie/lib-protocol-ilp"
import { createStore } from "@dassie/lib-reactive"

enableMapSet()

export const OutstandingRequestsStore = () =>
  createStore(
    castImmutable(
      new Map<number | string, (result: IlpResponsePacket) => void>(),
    ),
  ).actions({
    addRequest: (
      requestId: number | string,
      resolve: (result: IlpResponsePacket) => void,
    ) =>
      produce((draft) => {
        draft.set(requestId, resolve)
      }),
    removeRequest: (requestId: number | string) =>
      produce((draft) => {
        draft.delete(requestId)
      }),
  })
