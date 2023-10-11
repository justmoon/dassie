import { useVirtualizer } from "@tanstack/react-virtual"
import {
  startTransition,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react"

import {
  Card,
  CardContent,
  CardHeader,
} from "@dassie/app-node/src/frontend/components/ui/card"
import { Input } from "@dassie/app-node/src/frontend/components/ui/input"
import { combine } from "@dassie/app-node/src/frontend/utils/class-helper"
import { useRemoteStore } from "@dassie/lib-reactive-trpc/client"

import { type IndexedLogLine, LogsStore } from "../../../common/stores/logs"
import { trpc } from "../../utils/trpc"
import LogLine from "./log-line"

export interface LogViewerProperties {
  filter?: (line: IndexedLogLine) => boolean
  className?: string | undefined
}

const LogViewer = ({
  filter: externalFilter,
  className,
}: LogViewerProperties) => {
  const outerReference = useRef<HTMLDivElement>(null)
  const [shouldStick, setShouldStick] = useState(true)
  const scrollPositionReference = useRef<number | undefined>(undefined)
  const [keywordFilter, setKeywordFilter] = useState("")
  const { logs } = useRemoteStore(trpc.ui.subscribeToLogs, LogsStore)
  const latestLogLine = logs.at(-1)
  const filteredLogs = useMemo(
    () =>
      logs.filter((item) => {
        if (externalFilter && !externalFilter(item)) return false

        if (keywordFilter) {
          for (const filterElement of keywordFilter.split(/\s+/)) {
            const isNegative = filterElement.startsWith("-")
            const keyword = isNegative ? filterElement.slice(1) : filterElement
            const isMatching = item.message.includes(keyword)

            if (isNegative == isMatching) {
              return false
            }
          }
        }

        return true
      }),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [keywordFilter, externalFilter, logs, latestLogLine],
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
    <Card
      className={combine("min-h-0 h-full grid grid-rows-[auto_1fr]", className)}
    >
      <CardHeader>
        <Input
          type="text"
          value={keywordFilter}
          placeholder="Filter"
          onChange={(event) => {
            startTransition(() => {
              setKeywordFilter(event.currentTarget.value)
            })
          }}
        />
      </CardHeader>
      <CardContent ref={outerReference} className="min-h-0 overflow-y-scroll">
        <div
          className="relative"
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
      </CardContent>
    </Card>
  )
}

export default LogViewer
