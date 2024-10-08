import type { StreamPacket } from "@dassie/lib-protocol-stream"
import { isFailure } from "@dassie/lib-type-utils"

import StreamFrameDetails from "./stream-frame-details"
import type { StreamPacketParseFailure } from "./use-parsed-stream-packet"

interface StreamPacketDetailsProperties {
  packet: StreamPacket | StreamPacketParseFailure | undefined
  response?: boolean | undefined
}

export default function StreamPacketDetails({
  packet,
  response,
}: StreamPacketDetailsProperties) {
  return (
    <div className="grid grid-cols-subgrid col-span-2">
      {packet ?
        isFailure(packet) ?
          <div className="text-red-300 col-span-2">{packet.reason}</div>
        : <>
            <div className="text-muted-foreground">
              {response ? "Received Amount" : "Expected Amount"}
            </div>
            <div>{String(packet.amount)}</div>
            <div className="text-muted-foreground">Sequence</div>
            <div>{String(packet.sequence)}</div>
            {packet.frames.length > 0 ?
              <div className="col-span-2 flex flex-col gap-1">
                <div className="text-muted-foreground">Frames</div>
                <div className="bg-slate-900 p-3 flex flex-col gap-3 rounded-xl">
                  {packet.frames.map((frame, index) => (
                    <StreamFrameDetails key={index} frame={frame} />
                  ))}
                </div>
              </div>
            : <div className="text-muted-foreground">No frames</div>}
          </>

      : null}
    </div>
  )
}
