import { isError } from "@dassie/lib-logger"

import NodeLink from "../shared/node-link/node-link"
import { ANSI_COLORS } from "./ansi-theme"

const MAX_STRING_LENGTH = 30

interface DataValueProperties {
  keyName?: string
  content: unknown
}

interface TypeInfo {
  color: string
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  formatter?: (value: any) => string | JSX.Element
}

const typeofOperator = (value: unknown) => typeof value

type RecognizedType =
  | ReturnType<typeof typeofOperator>
  | "array"
  | "null"
  | "error"

const CHAR_LETTER_LOWERCASE_N = 110 // "n"
const NODE_ID_LENGTH = 43
const NODE_ID_REGEX = /^n\d+_[\w-]+$/

const TYPES = {
  string: {
    color: ANSI_COLORS["ansi-bright-yellow"],
    formatter: (value: string) => {
      if (
        value.length === NODE_ID_LENGTH &&
        value.codePointAt(0) === CHAR_LETTER_LOWERCASE_N &&
        NODE_ID_REGEX.test(value)
      ) {
        const nodeShortId = value.slice(0, value.indexOf("_"))
        return (
          <span className="font-bold">
            <NodeLink id={nodeShortId} />
          </span>
        )
      }

      return value.length > MAX_STRING_LENGTH
        ? `${value.slice(0, MAX_STRING_LENGTH)}…`
        : value
    },
  },
  number: {
    color: ANSI_COLORS["ansi-bright-cyan"],
  },
  boolean: {
    color: ANSI_COLORS["ansi-bright-red"],
  },
  bigint: {
    color: ANSI_COLORS["ansi-bright-cyan"],
    formatter: (value: bigint) => value.toString() + "n",
  },
  null: {
    color: ANSI_COLORS["ansi-bright-red"],
  },
  array: {
    color: ANSI_COLORS["ansi-bright-magenta"],
    formatter: (value: unknown[]) => `Array(${value.length})`,
  },
  object: {
    color: ANSI_COLORS["ansi-bright-magenta"],
    formatter: () => `Object`,
  },
  symbol: {
    color: ANSI_COLORS["ansi-magenta"],
  },
  function: {
    color: ANSI_COLORS["ansi-magenta"],
  },
  undefined: {
    color: ANSI_COLORS["ansi-bright-red"],
  },
  error: {
    color: ANSI_COLORS["ansi-bright-red"],
    formatter: (value: Error) => value.stack ?? value.message,
  },
} satisfies {
  [typeIdentifier in RecognizedType]: TypeInfo
}

export const DataValue = ({ keyName, content }: DataValueProperties) => {
  let typeIdentifier: RecognizedType = typeof content

  if (typeIdentifier === "object") {
    if (Array.isArray(content)) {
      typeIdentifier = "array"
    } else if (isError(content)) {
      typeIdentifier = "error"
    } else if (content === null) {
      typeIdentifier = "null"
    }
  }

  const typeInfo: TypeInfo = TYPES[typeIdentifier]

  const value =
    "formatter" in typeInfo ? typeInfo.formatter(content) : String(content)

  // eslint-disable-next-line no-console
  const onClick = () => console.log(content)

  return keyName ? (
    <span
      onClick={onClick}
      className={`bg-dark-100 rounded-1 text-xs px-1 inline-block cursor-pointer`}
    >
      <span className="font-sans text-gray-3">{keyName}=</span>
      <span style={{ color: typeInfo.color }}>{value}</span>
    </span>
  ) : (
    <span
      onClick={onClick}
      style={{ color: typeInfo.color }}
      className={`bg-dark-100 rounded-1 text-xs px-1 inline-block cursor-pointer`}
    >
      {value}
    </span>
  )
}
