/* eslint-disable unicorn/prefer-code-point */
import Anser from "anser"
import { Fragment } from "react"

import { isError } from "@dassie/lib-logger"
import { isObject } from "@dassie/lib-type-utils"

import { ANSI_COLORS, ANSI_DECORATIONS } from "./ansi-theme"
import { DataValue } from "./data-value"
import { PrimaryError } from "./primary-error"

export interface ParseLogParameters {
  message: string
  parameters: unknown[]
}

const CHAR_CODE_PERCENT = 37

const FORMATTERS = new Set([
  115, // s
  106, // j
  100, // d
  79, // O
  111, // o
  105, // i
  102, // f
  99, // c
])

const parseAnsi = (key: number, content: string): JSX.Element[] => {
  return Anser.ansiToJson(content, {
    json: true,
    use_classes: true,
    remove_empty: true,
  }).map((span, index) => (
    <span
      key={`${key}-${index}`}
      style={{
        color:
          span.fg ?
            ANSI_COLORS[span.fg as keyof typeof ANSI_COLORS]
          : undefined,
        backgroundColor:
          span.bg ?
            ANSI_COLORS[span.bg as keyof typeof ANSI_COLORS]
          : undefined,
      }}
      className={span.decorations
        .map((decoration) => ANSI_DECORATIONS[decoration])
        .join(" ")}
    >
      {span.content}
    </span>
  ))
}

const MAX_PROPERTIES_PRIMARY_OBJECT = 10

export const LogMessage = ({ message, parameters }: ParseLogParameters) => {
  const remainingParameters = [...parameters]
  const elements = [] as JSX.Element[]
  let lastChunkEnd = 0
  let unique = 0

  for (let index = 0; index < message.length; index++) {
    if (message.charCodeAt(index) !== CHAR_CODE_PERCENT) continue

    const nextCharacter = message.charCodeAt(index + 1)
    if (nextCharacter === CHAR_CODE_PERCENT) {
      elements.push(...parseAnsi(unique++, message.slice(lastChunkEnd, index)))
      lastChunkEnd = index + 2
      continue
    }

    if (!FORMATTERS.has(nextCharacter)) continue

    const parameter = remainingParameters.shift()
    elements.push(
      ...parseAnsi(unique++, message.slice(lastChunkEnd, index)),
      <DataValue key={unique++} content={parameter} />,
    )
    lastChunkEnd = index + 2
  }

  if (
    remainingParameters.length === 1 &&
    isObject(remainingParameters[0]) &&
    Object.prototype.toString.call(remainingParameters[0]) ===
      "[object Object]" &&
    Object.getOwnPropertySymbols(remainingParameters[0]).length === 0 &&
    Object.getOwnPropertyNames(remainingParameters[0]).length <
      MAX_PROPERTIES_PRIMARY_OBJECT
  ) {
    let primaryObject = remainingParameters[0]
    let error: Error | undefined
    if (isError(primaryObject["error"])) {
      const { error: extractedError, ...primaryObjectWithoutError } =
        primaryObject
      error = extractedError
      primaryObject = primaryObjectWithoutError
    }

    elements.push(
      ...parseAnsi(unique++, message.slice(lastChunkEnd)),
      ...Object.entries(primaryObject).map(([key, value]) => (
        <Fragment key={key}>
          {" "}
          <DataValue keyName={key} content={value} />
        </Fragment>
      )),
    )

    if (error) {
      elements.push(<PrimaryError key="error" error={error} />)
    }
  } else {
    // Remaining log message and parameters
    elements.push(
      ...parseAnsi(unique++, message.slice(lastChunkEnd)),
      ...remainingParameters.map((parameter) => (
        <Fragment key={unique++}>
          {" "}
          <DataValue content={parameter} />
        </Fragment>
      )),
    )
  }
  return <>{elements}</>
}
