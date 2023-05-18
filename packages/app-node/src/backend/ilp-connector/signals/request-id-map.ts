import { createSignal } from "@dassie/lib-reactive"

import { IlpPreparePacket, IlpType } from "../ilp-packet-codec"

interface RequestIdMapEntry {
  sourceAddress: string
  sourceRequestId: number
  preparePacket: IlpPreparePacket & { type: typeof IlpType.Prepare }
}

export const requestIdMapSignal = () =>
  createSignal(new Map<number, RequestIdMapEntry>())
