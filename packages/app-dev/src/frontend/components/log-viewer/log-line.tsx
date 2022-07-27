import LinkifyIt from "linkify-it"
import { Link } from "wouter"

import { selectBySeed } from "@xen-ilp/lib-logger"

import type { IndexedLogLine } from "../../../backend/features/logs"
import { COLORS } from "../../constants/palette"
import { startupTime } from "../../signals/startup-time"
import { AnsiSpan, AnsiSpanProperties } from "../utilities/ansi-span"

const linkify = new LinkifyIt()

export const linkifyText = (text: string) => {
  const matches = linkify.match(text) ?? []

  const elements = []
  let lastIndex = 0

  for (const match of matches) {
    if (match.index > lastIndex) {
      elements.push(text.slice(lastIndex, match.index))
    }

    const link = (
      <a
        key={match.index}
        href={match.url}
        target="blank"
        className="underline-gray-400 hover:underline"
      >
        {match.text}
      </a>
    )
    elements.push(link)

    lastIndex = match.lastIndex
  }

  if (text.length > lastIndex) {
    elements.push(text.slice(lastIndex))
  }

  return elements
}

export const DataValue = ({
  content,
  ...otherProperties
}: AnsiSpanProperties) => (
  <AnsiSpan content={content.split(/[\n ]/)[0]!} {...otherProperties} />
)

const LogLine = (log: IndexedLogLine) => {
  return (
    <div className="flex text-xs py-0.5" style={{ order: -log.index }}>
      <div className="font-mono flex-shrink-0 text-right px-2 text-gray-400 w-20">
        {((Number(new Date(log.date)) - (startupTime() ?? 0)) / 1000).toFixed(
          3
        )}
      </div>
      <div
        className="font-bold flex-shrink-0 text-center w-8"
        style={{ color: selectBySeed(COLORS, log.node) }}
      >
        <Link href={`/nodes/${log.node}`}>{log.node}</Link>
      </div>
      <pre className="font-mono px-2 break-all">
        <span style={{ color: selectBySeed(COLORS, log.component) }}>
          {log.component}
        </span>{" "}
        <span>
          <AnsiSpan content={log.message}></AnsiSpan>
        </span>
        {Object.entries(log.data ?? {}).map(([key, value]) =>
          key !== "error" ? (
            <span
              key={key}
              className="bg-dark-100 rounded-1 text-xs ml-1 py-0.5 px-1"
            >
              <span className="font-sans text-gray-400">{key}=</span>
              <span>
                <DataValue content={value} />
              </span>
            </span>
          ) : null
        )}
        {log.data?.["error"] ? (
          <p className="rounded-md bg-dark-300 mt-2 mb-4 p-2">
            <AnsiSpan content={log.data["error"]} />
          </p>
        ) : null}
      </pre>
    </div>
  )
}

export default LogLine
