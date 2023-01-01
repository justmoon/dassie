import LinkifyIt from "linkify-it"
import { Link } from "wouter"

import { selectBySeed } from "@dassie/lib-logger"

import type { IndexedLogLine } from "../../../backend/features/logs"
import { COLORS } from "../../constants/palette"
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

const DATA_PREVIEW_MAX_LENGTH = 30

export const DataValue = ({
  content,
  ...otherProperties
}: AnsiSpanProperties) => {
  const endOffset = Math.min(
    ...[DATA_PREVIEW_MAX_LENGTH, content.indexOf("\n")].filter((a) => a !== -1)
  )
  let abbreviatedContent = content.slice(0, endOffset)
  if (abbreviatedContent.length !== content.length) {
    abbreviatedContent += "…"
  }
  return <AnsiSpan content={abbreviatedContent} {...otherProperties} />
}

interface LogLineProperties {
  log: IndexedLogLine
}

const LogLine = ({ log }: LogLineProperties) => {
  return (
    <div className="flex text-xs py-0.5" style={{ order: -log.index }}>
      <div className="font-mono flex-shrink-0 text-right px-2 text-gray-400 w-20">
        {(log.relativeTime / 1000).toFixed(3)}
      </div>
      <div
        className="font-bold flex-shrink-0 text-center w-8"
        style={{ color: selectBySeed(COLORS, log.node) }}
      >
        <Link href={`/nodes/${log.node}`}>{log.node}</Link>
      </div>
      <pre className="font-mono px-2 whitespace-pre-wrap">
        <span style={{ color: selectBySeed(COLORS, log.component) }}>
          {log.component}
        </span>{" "}
        <span>
          <AnsiSpan content={log.message}></AnsiSpan>
        </span>
        {Object.entries(log.data ?? {}).map(([key, value]) =>
          key === "error" ? null : (
            <div
              key={key}
              className="bg-dark-100 rounded-1 text-xs ml-1 px-1 inline-block"
            >
              <span className="font-sans text-gray-400">{key}=</span>
              <span>
                <DataValue content={value} />
              </span>
            </div>
          )
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
