import { ArrowLeftIcon, ArrowRightIcon } from "lucide-react"
import { useState } from "react"

import { combine } from "@dassie/app-node/src/frontend/utils/class-helper"
import {
  IlpErrorCode,
  type IlpPreparePacket,
  type IlpResponsePacket,
  IlpType,
  humanReadableIlpErrors,
} from "@dassie/lib-protocol-ilp"
import type { PskEnvironment } from "@dassie/lib-protocol-stream"

import AmountTooLargeResponseDetails from "./AmountTooLargeResponseDetails"
import IlpPacketDetails from "./IlpPacketDetails"
import StreamPacketDetails from "./StreamPacketDetails"
import { useParsedStreamPacket } from "./use-parsed-stream-packet"

interface StreamPacketLogEntryProperties {
  packet: IlpPreparePacket
  response?: IlpResponsePacket | undefined
  pskEnvironment?: PskEnvironment | undefined
}

export default function StreamPacketLogEntry({
  packet,
  response,
  pskEnvironment,
}: StreamPacketLogEntryProperties) {
  const sender =
    packet.destination.startsWith("test.server") ? "client" : "server"

  const [isExpanded, setIsExpanded] = useState(false)

  const requestStreamPacket =
    pskEnvironment && useParsedStreamPacket(packet.data, pskEnvironment)
  const responseStreamPacket =
    pskEnvironment && useParsedStreamPacket(response?.data.data, pskEnvironment)

  const ArrowIcon = sender === "client" ? ArrowRightIcon : ArrowLeftIcon
  // response = undefined
  return (
    <div
      className={combine("flex flex-col bg-slate-800 rounded-lg overflow-clip")}
    >
      <div
        className="flex flex-row items-center justify-between cursor-pointer hover:bg-slate-700 transition-colors"
        onClick={() => setIsExpanded((value) => !value)}
      >
        <div
          className={combine(
            "px-3 py-2 flex gap-3 items-center",

            { client: "flex-row", server: "flex-row-reverse" }[sender],
          )}
        >
          <ArrowIcon
            className={combine(
              "size-4 rounded-full stroke-6",
              !!response ?
                response.type === IlpType.Fulfill ?
                  "text-green-500"
                : "text-red-500"
              : "text-slate-400",
            )}
          />
          <div className="font-bold">
            {!!response ?
              response.type === IlpType.Fulfill ?
                "Fulfilled"
              : <div className="flex flex-row gap-3">
                  <div>Rejected</div>
                  <div className="text-red-300">
                    {response.data.code}{" "}
                    {humanReadableIlpErrors[
                      response.data.code as IlpErrorCode
                    ] ?? "Unknown Error"}
                  </div>
                </div>

            : "Prepared"}
          </div>
          {isExpanded ? null : String(packet.amount)}
        </div>
        <div
          className={combine(
            "size-0 select-none transition-transform border-t-10 border-l-6 border-r-6 border-transparent border-t-slate-400 mr-3",
            isExpanded && "rotate-180",
          )}
        />
      </div>

      <div
        className={combine(
          "overflow-y-clip h-0 transition-height",
          isExpanded && "h-[calc-size(auto)]",
        )}
      >
        <div className="grid grid-cols-[auto_auto_1fr] gap-3 px-3 py-2 border-t border-slate-700">
          <div className="">ILP</div>
          <IlpPacketDetails packet={packet} />
          <div className="">STREAM Request</div>
          <StreamPacketDetails packet={requestStreamPacket} />
          {response &&
            ((
              response.type === IlpType.Reject &&
              response.data.code === IlpErrorCode.F08_AMOUNT_TOO_LARGE
            ) ?
              <>
                <div className="">F08 Response Data</div>
                <AmountTooLargeResponseDetails data={response.data.data} />
              </>
            : <>
                <div className="">STREAM Response</div>
                <StreamPacketDetails packet={responseStreamPacket} response />
              </>)}
        </div>
      </div>
    </div>
  )
}
