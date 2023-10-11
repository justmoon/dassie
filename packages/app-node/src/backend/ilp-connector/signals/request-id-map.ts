import { createSignal } from "@dassie/lib-reactive"

import { PreparedIlpPacketEvent } from "../topics/prepared-ilp-packet"

export const RequestIdMapSignal = () =>
  createSignal(new Map<number, PreparedIlpPacketEvent>())
