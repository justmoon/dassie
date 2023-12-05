import { PreparedIlpPacketEvent } from "../topics/prepared-ilp-packet"
import { IlpAddress } from "../types/ilp-address"

export type PendingPacketsKey = `${IlpAddress}#${number | string}`

export const PendingPacketsMap = () =>
  new Map<PendingPacketsKey, PreparedIlpPacketEvent>()
