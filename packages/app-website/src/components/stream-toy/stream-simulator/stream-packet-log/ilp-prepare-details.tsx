import {
  type IlpPreparePacket,
  interledgerTimeToTimestamp,
} from "@dassie/lib-protocol-ilp"

interface IlpPacketDetailsProperties {
  packet: IlpPreparePacket
}

export default function IlpPacketDetails({
  packet,
}: IlpPacketDetailsProperties) {
  return (
    <div className="grid grid-cols-subgrid col-span-2">
      <div className="text-muted-foreground">Amount</div>
      <div>{String(packet.amount)}</div>
      <div className="text-muted-foreground">Destination</div>
      <div>{packet.destination}</div>
      <div className="text-muted-foreground">Expiry</div>
      <div>
        {new Date(interledgerTimeToTimestamp(packet.expiresAt)).toISOString()}
      </div>
      <div className="text-muted-foreground">Data</div>
      <div>{packet.data.length} bytes</div>
    </div>
  )
}
