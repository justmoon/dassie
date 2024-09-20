import type { ReactNode } from "react"

import { ANSI_COLORS } from "./ansi-theme"

export type FormatDefinition = {
  [typeIdentifier in RecognizedType]: TypeInfo<
    {
      string: string
      number: number
      bigint: bigint
      boolean: boolean
      symbol: symbol
      undefined: undefined
      object: object
      // eslint-disable-next-line @typescript-eslint/no-unsafe-function-type
      function: Function
      array: unknown[]
      null: null
      error: Error
    }[typeIdentifier]
  >
}

export interface TypeInfo<T = unknown> {
  color: string
  formatter?: (value: T) => ReactNode
}

const _typeofOperator = (value: unknown) => typeof value

export type RecognizedType =
  | ReturnType<typeof _typeofOperator>
  | "array"
  | "null"
  | "error"

export const MAX_STRING_LENGTH = 30

export const DEFAULT_FORMAT = {
  string: {
    color: ANSI_COLORS["ansi-bright-yellow"],
    formatter: (value) => {
      if (value.length > MAX_STRING_LENGTH) {
        return value.slice(0, MAX_STRING_LENGTH) + "…"
      }

      return value
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
    formatter: (value) => value.toString() + "n",
  },
  null: {
    color: ANSI_COLORS["ansi-bright-red"],
  },
  array: {
    color: ANSI_COLORS["ansi-bright-magenta"],
    formatter: (value) => `Array(${value.length})`,
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
    formatter: (value) => value.stack ?? value.message,
  },
} satisfies FormatDefinition
