import { hasAggregatedErrors } from "@dassie/lib-logger"
import { isError } from "@dassie/lib-type-utils"

import { combine } from "../../utils/class-helper"
import { DataValue } from "./data-value"
import { useLogViewerContext } from "./log-viewer"

export interface PrimaryErrorProperties {
  error: Error
}

interface FilePathStackFrameProperties {
  name?: string
  path: string
  row: number
  column: number
}

const FilePathStackFrame = ({
  name,
  path,
  row,
  column,
}: FilePathStackFrameProperties) => {
  const { openFile } = useLogViewerContext()
  const isNode = path.startsWith("node:")
  const isNodeModules = path.includes("node_modules")
  const fileLink =
    isNode ?
      <span>
        {path}:{row}:{column}
      </span>
    : <span
        className="text-blue-300 hover:underline cursor-pointer"
        onClick={() => {
          openFile?.(`${path}:${row}:${column}`)
        }}
      >
        {path}:{row}:{column}
      </span>
  return (
    <span className={combine({ "text-gray-400": isNode || isNodeModules })}>
      <span className="text-gray-400">{"  at "}</span>
      {name ?
        <span>
          {name} ({fileLink})
        </span>
      : <span>{fileLink}</span>}
    </span>
  )
}

interface UnknownStackFrameProperties {
  frame: string
}

const UnknownStackFrame = ({ frame }: UnknownStackFrameProperties) => {
  return <span className="text-gray-400">{frame}</span>
}

export const PrimaryError = ({ error }: PrimaryErrorProperties) => {
  const name = error.name || "Error"
  const message = error.message || ""
  const stack = error.stack?.split("\n").slice(1) ?? []

  const formattedCallsites = stack
    .map((line) => {
      {
        const match = /^ {4}at (.*) \((.*):(\d+):(\d+)\)$/.exec(line) as
          | [string, string, string, string, string]
          | null
        if (match) {
          return (
            <FilePathStackFrame
              name={match[1]}
              path={match[2]}
              row={Number.parseInt(match[3])}
              column={Number.parseInt(match[4])}
            />
          )
        }
      }
      {
        const match = /^ {4}at (.*):(\d+):(\d+)$/.exec(line) as
          | [string, string, string, string]
          | null
        if (match) {
          return (
            <FilePathStackFrame
              path={match[1]}
              row={Number.parseInt(match[2])}
              column={Number.parseInt(match[3])}
            />
          )
        }
      }

      // eslint-disable-next-line react/jsx-key
      return <UnknownStackFrame frame={line} />
    })
    .filter(Boolean)

  return (
    <>
      <div className={`bg-destructive rounded-2 text-xs my-2 p-3`}>
        <div className="text-destructive-foreground font-bold mb-1">
          <span>{name}</span>
          {message ? ": " : ""}
          <span>{message}</span>
        </div>
        <ul>
          {formattedCallsites.map((line, index) => (
            <li key={index}>{line}</li>
          ))}
        </ul>
      </div>
      {"cause" in error &&
        error.cause &&
        error !== error.cause &&
        !hasAggregatedErrors(error) &&
        (isError(error.cause) ?
          <PrimaryError error={error.cause} />
        : <DataValue keyName="cause" content={error.cause} />)}
      {hasAggregatedErrors(error) &&
        error.errors.map((error, index) => (
          <PrimaryError key={index} error={error} />
        ))}
    </>
  )
}
