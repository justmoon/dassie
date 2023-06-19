import cx from "classnames"

import { trpc } from "../../utils/trpc"

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
  const openFileMutation = trpc.ui.openFile.useMutation()
  const isNode = path.startsWith("node:")
  const isNodeModules = path.includes("node_modules")
  const fileLink = isNode ? (
    <span>
      {path}:{row}:{column}
    </span>
  ) : (
    <span
      className="text-blue-300 hover:underline cursor-pointer"
      onClick={() => openFileMutation.mutate(`${path}:${row}:${column}`)}
    >
      {path}:{row}:{column}
    </span>
  )
  return (
    <span className={cx({ "text-gray-400": isNode || isNodeModules })}>
      <span className="text-gray-400">{"  at "}</span>
      {name ? (
        <span>
          {name} ({fileLink})
        </span>
      ) : (
        <span>{fileLink}</span>
      )}
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
  const stack =
    error.stack ?? `${error.name || "Error"}: ${error.message || "no message"}`
  const lines = stack.split("\n")

  const [name, message] = lines[0]?.split(": ") ?? []

  const formattedCallsites = lines
    .slice(1)
    .map((line) => {
      {
        const match = line.match(/^ {4}at (.*) \((.*):(\d+):(\d+)\)$/) as
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
        const match = line.match(/^ {4}at (.*):(\d+):(\d+)$/) as
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
    <div className={`bg-dark-100 rounded-2 text-xs my-2 p-3`}>
      <div className="text-red-400 font-bold mb-1">
        <span>{name}</span>: <span>{message}</span>
      </div>
      <ul>
        {formattedCallsites.map((line, index) => (
          <li key={index}>{line}</li>
        ))}
      </ul>
    </div>
  )
}
