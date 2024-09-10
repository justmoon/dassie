import { useVirtualizer } from "@tanstack/react-virtual"
import {
  type ReactNode,
  createContext,
  startTransition,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react"

import { combine } from "../../utils/class-helper"
import { Card, CardContent, CardHeader } from "../ui/card"
import { Input } from "../ui/input"
import { DEFAULT_FORMAT, type FormatDefinition } from "./default-format"
import LogLine, { type ViewableLogLine } from "./log-line"

interface LogViewerContextValue {
  openFile: ((path: string) => void) | undefined
  renderNodeColumn: ((log: ViewableLogLine) => ReactNode) | undefined
  format: FormatDefinition
}

const DEFAULT_CONTEXT = {
  openFile: undefined,
  renderNodeColumn: undefined,
  format: DEFAULT_FORMAT,
}

const LogViewerContext = createContext<LogViewerContextValue>(DEFAULT_CONTEXT)

export const useLogViewerContext = () => useContext(LogViewerContext)

export const LogViewerProvider = ({
  children,
  ...value
}: Partial<LogViewerContextValue> & { children: ReactNode }) => (
  <LogViewerContext.Provider value={{ ...DEFAULT_CONTEXT, ...value }}>
    {children}
  </LogViewerContext.Provider>
)

export interface LogViewerProperties<TLogLine extends ViewableLogLine> {
  logs: TLogLine[]
  filter?: ((line: TLogLine) => boolean) | undefined
  className?: string | undefined
}

const LogViewer = <TLogLine extends ViewableLogLine>({
  logs,
  filter: externalFilter,
  className,
}: LogViewerProperties<TLogLine>) => {
  const outerReference = useRef<HTMLDivElement>(null)
  const [shouldStick, setShouldStick] = useState(true)
  const scrollPositionReference = useRef<number | undefined>(undefined)
  const [keywordFilter, setKeywordFilter] = useState("")
  const filteredLogs = useMemo(
    () =>
      logs.filter((item) => {
        if (externalFilter && !externalFilter(item)) return false

        if (keywordFilter) {
          for (const filterElement of keywordFilter.split(/\s+/)) {
            const isNegative = filterElement.startsWith("-")
            const keyword = isNegative ? filterElement.slice(1) : filterElement
            const isMatching =
              item.namespace.includes(keyword) || item.message.includes(keyword)

            if (isNegative == isMatching) {
              return false
            }
          }
        }

        return true
      }),
    [keywordFilter, externalFilter, logs],
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
