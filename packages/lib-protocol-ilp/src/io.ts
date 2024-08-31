import type { IlpPacket, IlpPreparePacket, IlpType } from "./schema"

export type IlpPacketHandler = (
  packet: IlpPreparePacket,
) => Promise<
  IlpPacket & { type: typeof IlpType.Fulfill | typeof IlpType.Reject }
>

/**
 * A standard way of expressing a two-way Interledger communication channel.
 */
export type IlpEndpoint = {
  readonly sendPacket: IlpPacketHandler
  readonly handlePackets: (handler: IlpPacketHandler) => () => void
}
