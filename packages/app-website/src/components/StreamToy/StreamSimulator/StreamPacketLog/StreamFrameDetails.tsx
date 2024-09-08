import { FrameType, type StreamFrame } from "@dassie/lib-protocol-stream"

interface StreamFrameProperties {
  frame: StreamFrame
}

export default function StreamFrameDetails({ frame }: StreamFrameProperties) {
  function renderFrameContents() {
    switch (frame.type) {
      case FrameType.StreamMoney: {
        return (
          <div>
            StreamMoney Stream {String(frame.data.streamId)} &mdash;{" "}
            {String(frame.data.shares)} shares
          </div>
        )
      }
      default: {
        return <div>Unknown frame type {frame.type}</div>
      }
    }
  }

  return (
    <div className="bg-slate-900 p-3 -ml-3 flex flex-col gap-1">
      {renderFrameContents()}
    </div>
  )
}
