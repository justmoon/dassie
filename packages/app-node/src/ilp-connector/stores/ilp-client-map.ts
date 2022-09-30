import produce, { enableMapSet } from "immer"
import type { Promisable } from "type-fest"

import { createStore } from "@dassie/lib-reactive"

import type { OutgoingIlpPacket } from "../topics/outgoing-ilp-packet"

export interface IlpClientInfo {
  id: string
  type: string
  sendPacket: (packet: OutgoingIlpPacket) => Promisable<void>
}

enableMapSet()

export const ilpClientMapStore = () =>
  createStore(new Map<string, IlpClientInfo>(), {
    addIlpClient: ({ id, type, sendPacket }: IlpClientInfo) =>
      produce((draft) => {
        draft.set(id, { id, type, sendPacket })
      }),
    removeIlpClient: (id: string) =>
      produce((draft) => {
        draft.delete(id)
      }),
  })

export const generateNewClientId = (ilpClientMap: Map<string, unknown>) => {
  let id

  do {
    id = Math.random().toString(36).slice(2, 8)
  } while (ilpClientMap.has(id))

  return id
}
