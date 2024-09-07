import type { PreparedIlpPacketEvent } from "../topics/prepared-ilp-packet"
import type { IlpAddress } from "../types/ilp-address"

export type PendingPacketsKey = `${IlpAddress}#${number | string}`

export const PendingPacketsMap = () =>
  new Map<PendingPacketsKey, PreparedIlpPacketEvent>()
