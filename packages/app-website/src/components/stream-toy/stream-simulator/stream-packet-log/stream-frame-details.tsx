import type { ReactNode } from "react"

import {
  ErrorCode,
  FrameType,
  type StreamFrame,
} from "@dassie/lib-protocol-stream"

const errorCodeMap = new Map<number, string>(
  Object.entries(ErrorCode).map(([name, code]) => [code, name]),
)

interface StreamFrameProperties {
  frame: StreamFrame
}

interface FrameWrapperProperties {
  children: ReactNode
}

function FrameWrapper({ children }: FrameWrapperProperties) {
  return <div className="flex flex-row gap-2">{children}</div>
}

interface FrameTypeBadgeProperties {
  children: ReactNode
}

function FrameTypeBadge({ children }: FrameTypeBadgeProperties) {
  return (
    <div className="text-xs bg-slate-800 text-slate-400 rounded-full px-2 py-1">
      {children}
    </div>
  )
}

export default function StreamFrameDetails({
  frame,
}: StreamFrameProperties): ReactNode {
  function renderFrameContents() {
    switch (frame.type) {
      case FrameType.ConnectionClose: {
        return (
          <FrameWrapper>
            <FrameTypeBadge>ConnectionClose</FrameTypeBadge>
            <div>
              Error Code:{" "}
              {errorCodeMap.get(frame.data.errorCode) ??
                `Unknown (0x${frame.data.errorCode.toString(16).padStart(2, "0")})`}{" "}
              Error Message: {frame.data.errorMessage}
            </div>
          </FrameWrapper>
        )
      }
      case FrameType.ConnectionNewAddress: {
        return (
          <FrameWrapper>
            <FrameTypeBadge>ConnectionNewAddress</FrameTypeBadge>
            <div>{frame.data.sourceAccount}</div>
          </FrameWrapper>
        )
      }
      case FrameType.ConnectionAssetDetails: {
        return (
          <FrameWrapper>
            <FrameTypeBadge>ConnectionAssetDetails</FrameTypeBadge>
            <div>
              Asset Code: {frame.data.sourceAssetCode} Scale:{" "}
              {frame.data.sourceAssetScale}
            </div>
          </FrameWrapper>
        )
      }
      case FrameType.StreamMoney: {
        return (
          <FrameWrapper>
            <FrameTypeBadge>StreamMoney</FrameTypeBadge>
            <div>
              Stream ID {String(frame.data.streamId)}:{" "}
              {String(frame.data.shares)} shares
            </div>
          </FrameWrapper>
        )
      }
      default: {
        return <div>Unknown frame type {frame.type}</div>
      }
    }
  }

  return renderFrameContents()
}
