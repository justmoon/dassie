import type { Promisable } from "type-fest"

import { createSignal } from "@dassie/lib-reactive"

import type { OutgoingIlpPacket } from "../topics/outgoing-ilp-packet"
import PrefixMap from "../utils/prefix-map"

export interface IlpClientInfo {
  prefix: string
  type: string
  sendPacket: (packet: OutgoingIlpPacket) => Promisable<void>
}

export const ilpRoutingTableSignal = () =>
  createSignal(new PrefixMap<IlpClientInfo>())
