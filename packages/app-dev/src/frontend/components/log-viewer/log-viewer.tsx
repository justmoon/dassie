import { useVirtualizer } from "@tanstack/react-virtual"
import {
  startTransition,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react"

import { useSig } from "@dassie/lib-reactive-trpc/client"

import type { IndexedLogLine } from "../../../common/stores/logs"
import { remoteLogsStore } from "../../remote-signals/logs"
import LogLine from "./log-line"

export interface LogViewerProperties {
  filter?: (line: IndexedLogLine) => boolean
}

const LogViewer = ({ filter: externalFilter }: LogViewerProperties) => {
  const outerReference = useRef<HTMLDivElement>(null)
  const [shouldStick, setShouldStick] = useState(true)
  const scrollPositionReference = useRef<number | undefined>(undefined)
  const [keywordFilter, setKeywordFilter] = useState("")
  const sig = useSig()
  const logs = sig.get(remoteLogsStore)?.logs
  const latestLogLine = logs?.[logs.length - 1]
  const filteredLogs = useMemo(
    () =>
      logs?.filter((item) => {
        if (externalFilter && !externalFilter(item)) return false

        if (keywordFilter) {
          const data = item.data
            ? Object.entries(item.data)
                .map(([key, value]) => `${key}=${value}`)
                .join(" ")
            : ""
          for (const filterElement of keywordFilter.split(/\s+/)) {
            const isNegative = filterElement.startsWith("-")
            const keyword = isNegative ? filterElement.slice(1) : filterElement
            const isMatching =
              item.message.includes(keyword) ||
              item.component.includes(keyword) ||
              data.includes(keyword)

            if (isNegative == isMatching) {
              return false
            }
          }
        }

        return true
      }) ?? [],
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [keywordFilter, externalFilter, logs, latestLogLine]
  )

  const virtualizer = useVirtualizer({
    count: filteredLogs.length,
    getScrollElement: () => outerReference.current,
    estimateSize: () => 21,
    getItemKey: (index) => filteredLogs[index]!.index,
  })

  useEffect(() => {
    const outerElement = outerReference.current
    if (!outerElement) return

    const onScroll = () => {
      if (scrollPositionReference.current !== outerElement.scrollTop) {
        const newShouldStick =
          outerElement.scrollHeight - outerElement.scrollTop ===
          outerElement.clientHeight

        setShouldStick(newShouldStick)
      }
    }
    outerElement.addEventListener("scroll", onScroll)

    return () => {
      outerElement.removeEventListener("scroll", onScroll)
    }
  }, [outerReference, shouldStick, setShouldStick])

  useLayoutEffect(() => {
    const outerElement = outerReference.current

    if (!outerElement) return

    if (shouldStick) {
      outerElement.scrollTop =
        outerElement.scrollHeight - outerElement.clientTop
    }

    scrollPositionReference.current = outerElement.scrollTop
  })

  return (
    <div className="h-full w-full flex flex-col gap-4 min-h-0">
      <div className="flex items-center gap-2">
        <input
          type="text"
          value={keywordFilter}
          placeholder="Filter"
          onChange={(event) => {
            startTransition(() => {
              setKeywordFilter(event.currentTarget.value)
            })
          }}
          className="bg-gray-700 border border-gray-600 text-white text-sm rounded-lg placeholder-gray-400 focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5"
        />
      </div>
      <div ref={outerReference} className="flex-1 overflow-y-scroll">
        <div
          className="w-full relative"
          style={{ height: virtualizer.getTotalSize() }}
        >
          {virtualizer.getVirtualItems().map((virtualItem) => {
            const logLine = filteredLogs[virtualItem.index]
            if (!logLine) return null
            return (
              <div
                key={virtualItem.key}
                data-index={virtualItem.index}
                ref={virtualizer.measureElement}
                className="w-full top-0 left-0 absolute"
                style={{ transform: `translateY(${virtualItem.start}px)` }}
              >
                <LogLine log={logLine} />
              </div>
            )
          })}
        </div>
      </div>
    </div>
  )
}

export default LogViewer
