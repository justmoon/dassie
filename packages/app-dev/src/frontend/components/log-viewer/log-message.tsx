/* eslint-disable unicorn/prefer-code-point */
import Anser from "anser"

import { isError, selectBySeed } from "@dassie/lib-logger"
import { isObject } from "@dassie/lib-type-utils"

import { COLORS } from "../../constants/palette"
import { ANSI_COLORS, ANSI_DECORATIONS } from "./ansi-theme"
import DataValue from "./data-value"
import { PrimaryError } from "./primary-error"

export interface ParseLogParameters {
  message: string
  parameters: unknown[]
}

const CHAR_CODE_PERCENT = 37
const CHAR_CODE_COLON = 58
const CHAR_CODE_SPACE = 32
const CHAR_CODE_ESCAPE = 27

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
        color: span.fg
          ? ANSI_COLORS[span.fg as keyof typeof ANSI_COLORS]
          : undefined,
        backgroundColor: span.bg
          ? ANSI_COLORS[span.bg as keyof typeof ANSI_COLORS]
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
  let sawColon = false
  let sawSpace = false

  for (let index = 0; index < message.length; index++) {
    // Check for a component name (identified by a string containing a colon but not as the first or last character)
    if (lastChunkEnd === 0 && !sawSpace) {
      if (message.charCodeAt(index) === CHAR_CODE_COLON) {
        sawColon = true
      }
      if (message.charCodeAt(index) === CHAR_CODE_SPACE) {
        sawSpace = true
        if (
          sawColon &&
          message.charCodeAt(index) === CHAR_CODE_SPACE &&
          message.charCodeAt(index - 1) !== CHAR_CODE_COLON &&
          message.charCodeAt(0) !== CHAR_CODE_COLON &&
          message.charCodeAt(0) !== CHAR_CODE_ESCAPE
        ) {
          const componentName = message.slice(0, index)
          elements.push(
            <span
              key={unique++}
              style={{ color: selectBySeed(COLORS, componentName) }}
            >
              {componentName}
            </span>
          )
          lastChunkEnd = index
          continue
        }
      }
    }
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
      <DataValue key={unique++} content={parameter} />
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
      ...parseAnsi(unique++, message.slice(lastChunkEnd, message.length)),
      ...Object.entries(primaryObject).map(([key, value]) => (
        <DataValue key={key} keyName={key} content={value} />
      ))
    )

    if (error) {
      elements.push(<PrimaryError key="error" error={error} />)
    }
  } else {
    // Remaining log message and parameters
    elements.push(
      ...parseAnsi(unique++, message.slice(lastChunkEnd, message.length)),
      ...remainingParameters.map((parameter) => (
        <DataValue key={unique++} content={parameter} />
      ))
    )
  }
  return <>{elements}</>
}
