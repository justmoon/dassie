import { createSignal } from "@dassie/lib-reactive"

import { PreparedIlpPacketEvent } from "../topics/prepared-ilp-packet"

export const requestIdMapSignal = () =>
  createSignal(new Map<number, PreparedIlpPacketEvent>())
