import { ReactNode } from "react"
import reactStringReplace from "react-string-replace"

import { isError } from "@dassie/lib-logger"

import { TEST_NODE_VANITY_IDS } from "../../../backend/constants/vanity-nodes"
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
  formatter?: (value: any) => ReactNode
}

const typeofOperator = (value: unknown) => typeof value

type RecognizedType =
  | ReturnType<typeof typeofOperator>
  | "array"
  | "null"
  | "error"

const vanityNodeIdsSet = new Set(TEST_NODE_VANITY_IDS)

const NODE_ID_REGEX = /(d\d{1,4}_[\w-]{25,30})/

const TYPES = {
  string: {
    color: ANSI_COLORS["ansi-bright-yellow"],
    formatter: (value: string) => {
      const chunks = reactStringReplace(
        value,
        NODE_ID_REGEX,
        (match, index) => {
          if (!vanityNodeIdsSet.has(match)) {
            return match
          }

          const nodeShortId = match.slice(0, match.indexOf("_"))
          return (
            <span key={index} className="font-bold">
              <NodeLink id={nodeShortId} />
            </span>
          )
        },
      )

      let totalLength = 0

      for (let index = 0; index < chunks.length; index++) {
        const chunk = chunks[index]

        if (typeof chunk === "string") {
          if (totalLength + chunk.length > MAX_STRING_LENGTH) {
            chunks[index] =
              chunk.slice(0, MAX_STRING_LENGTH - totalLength) + "…"

            return chunks.slice(0, index + 1)
          }

          totalLength += chunk.length
        }
      }

      return chunks
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
