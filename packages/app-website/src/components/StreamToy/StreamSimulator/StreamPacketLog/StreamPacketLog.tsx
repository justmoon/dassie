import type {
  IlpPreparePacket,
  IlpResponsePacket,
} from "@dassie/lib-protocol-ilp"
import type { PskEnvironment } from "@dassie/lib-protocol-stream"

import StreamPacketLogEntry from "./StreamPacketLogEntry"

interface StreamPacketLogProperties {
  packets: IndexedPreparePacketEvent[]
  responses: Map<IlpPreparePacket, IlpResponsePacket>
  pskEnvironment?: PskEnvironment | undefined
}

export interface IndexedPreparePacketEvent {
  packet: IlpPreparePacket
  index: number
  sender: string
}

export default function StreamPacketLog({
  packets,
  responses,
  pskEnvironment,
}: StreamPacketLogProperties) {
  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-row justify-between">
        <div>
          <div className="font-bold">Sender</div>
          <div className="text-sm text-muted-foreground">test.client</div>
        </div>
        <div>
          <div className="font-bold">Receiver</div>
          <div className="text-sm text-muted-foreground">test.server</div>
        </div>
      </div>
      {packets
        .filter(({ packet: { destination } }) => destination !== "peer.config")
        .map((event) => (
          <StreamPacketLogEntry
            key={event.index}
            packet={event.packet}
            response={responses.get(event.packet)}
            pskEnvironment={pskEnvironment}
          />
        ))}
    </div>
  )
}
