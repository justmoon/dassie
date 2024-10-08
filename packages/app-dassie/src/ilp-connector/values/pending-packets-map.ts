import type { PreparedIlpPacketEvent } from "../topics/prepared-ilp-packet"

export type PendingPacketsKey = `${string}#${number | string}`

export const PendingPacketsMap = () =>
  new Map<PendingPacketsKey, PreparedIlpPacketEvent>()
