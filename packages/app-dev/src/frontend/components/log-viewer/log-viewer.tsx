import { useVirtualizer } from "@tanstack/react-virtual"
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react"

import { useStore } from "@dassie/lib-reactive-react"

import type { IndexedLogLine } from "../../../backend/features/logs"
import { remoteLogsStore } from "../../remote-stores/logs"
import LogLine from "./log-line"

export interface LogViewerProperties {
  filter?: (line: IndexedLogLine) => boolean
}

const LogViewer = ({ filter }: LogViewerProperties) => {
  const outerReference = useRef<HTMLDivElement>(null)
  const [shouldStick, setShouldStick] = useState(true)
  const scrollPositionReference = useRef<number | undefined>(undefined)
  const logs = useStore(remoteLogsStore)?.logs ?? []
  const filteredLogs = useMemo(
    () => (filter ? logs.filter((item) => filter(item)) : logs),
    [filter, logs]
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
    <div ref={outerReference} className="h-full w-full overflow-y-scroll">
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
              ref={virtualItem.measureElement}
              className="w-full top-0 left-0 absolute"
              style={{ transform: `translateY(${virtualItem.start}px)` }}
            >
              <LogLine log={logLine} />
            </div>
          )
        })}
      </div>
    </div>
  )
}

export default LogViewer
