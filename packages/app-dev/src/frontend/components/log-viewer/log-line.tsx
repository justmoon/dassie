import { Link } from "wouter"

import { selectBySeed } from "@dassie/lib-logger"
import { isObject } from "@dassie/lib-type-utils"

import type { IndexedLogLine } from "../../../common/stores/logs"
import { COLORS } from "../../constants/palette"
import { AnsiSpan } from "../utilities/ansi-span"

const MAX_STRING_LENGTH = 30

interface DataValueProperties {
  content: unknown
}

export const DataValue = ({ content }: DataValueProperties) => {
  if (typeof content === "string") {
    if (content.length < MAX_STRING_LENGTH) {
      return <span>{content}</span>
    }

    return <span>{`${content.slice(0, MAX_STRING_LENGTH)}…`}</span>
  } else if (typeof content === "number") {
    return <span>{content}</span>
  } else if (typeof content === "boolean") {
    return <span>{content ? "true" : "false"}</span>
  } else if (typeof content === "bigint") {
    return <span>{content.toString()}</span>
  } else if (content === null) {
    return <span>null</span>
  } else if (Array.isArray(content)) {
    return <span>Array({content.length})</span>
  } else if (isObject(content)) {
    return <span>Object</span>
  }

  return <span>Other</span>
}

interface LogLineProperties {
  log: IndexedLogLine
}

const LogLine = ({ log }: LogLineProperties) => {
  const renderedParameters = [] as JSX.Element[]

  if (
    log.parameters.length === 1 &&
    isObject(log.parameters[0]) &&
    Object.prototype.toString.call(log.parameters[0]) === "[object Object]" &&
    Object.getOwnPropertySymbols(log.parameters[0]).length === 0
  ) {
    for (const [key, parameter] of Object.entries(
      log.parameters[0] as Record<string, unknown>
    )) {
      renderedParameters.push(
        <div
          key={renderedParameters.length}
          className="bg-dark-100 rounded-1 text-xs ml-1 px-1 inline-block"
        >
          <span className="font-sans text-gray-400">{key}=</span>
          <DataValue key={renderedParameters.length} content={parameter} />
        </div>
      )
    }
  } else {
    for (const parameter of log.parameters) {
      renderedParameters.push(
        <div
          key={renderedParameters.length}
          className="bg-dark-100 rounded-1 text-xs ml-1 px-1 inline-block"
        >
          <DataValue content={parameter} />
        </div>
      )
    }
  }

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
        <span>
          <AnsiSpan content={log.message}></AnsiSpan>
        </span>
        {renderedParameters}
      </pre>
    </div>
  )
}

export default LogLine
