import type {
  IlpPacket,
  IlpPreparePacket,
  IlpType,
} from "@dassie/lib-protocol-ilp"

export type PacketHandler = (
  packet: IlpPreparePacket,
) => Promise<
  IlpPacket & { type: typeof IlpType.Fulfill | typeof IlpType.Reject }
>
