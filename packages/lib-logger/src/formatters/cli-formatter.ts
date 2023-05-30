import ansiEscapes from "ansi-escapes"
import chalk from "chalk"
import isGeneratorFunction from "is-generator-function"
import stringWidth from "string-width"

import { isObject } from "@dassie/lib-type-utils"

import type { Formatter, LogEventFormatter } from "../types/formatter"
import type { LogMessage } from "../types/log-event"
import { isAsyncFunction } from "../utils/is-async-function"
import { isError } from "../utils/is-error"
import { selectBySeed } from "../utils/select-by-seed"

export interface Theme {
  levelPrefixes: {
    debug: string
    info: string
    warn: string
    error: string
  }
  colors: readonly [Formatter<string>, ...Formatter<string>[]]
  values: {
    string: Formatter<string>
    number: Formatter<number>
    bigint: Formatter<bigint>
    boolean: Formatter<boolean>
    symbol: Formatter<symbol>
    function: (type: string, name: string) => string

    undefined: string
    null: string
    nan: string
    positiveInfinity: string
    negativeInfinity: string

    date: Formatter<string>
    regexp: Formatter<string>
    weakSet: string
    weakMap: string

    truncatedString: Formatter<{ short: string; original: string }>

    key: Formatter<string>
    symbolKey: Formatter<symbol>
    quotedKey: Formatter<string>
    inlineKey: Formatter<string>

    emptyArray: string
    emptyObject: string
    emptyMap: string
    emptySet: string

    accessor: Formatter<string>
    anonymousFunctionName: string
    hexBytes: Formatter<string>
  }
  structures: {
    parameterObjectStart: string
    parameterObjectKeySeparator: string
    parameterObjectElementSeparator: string
    parameterObjectEnd: string

    arrayInlineStart: string
    arrayInlineSeparator: string
    arrayInlineEnd: string

    objectConstructor: (name: string) => string
    objectConstructorWithSize: (name: string, size: number) => string
    objectInlineStart: string
    objectInlineKeySeparator: string
    objectInlineElementSeparator: string
    objectInlineEnd: string

    mapInlineStart: string
    mapInlineKeySeparator: string
    mapInlineElementSeparator: string
    mapInlineEnd: string

    setInlineStart: string
    setInlineSeparator: string
    setInlineEnd: string

    hexBytesInlineStart: string
    hexBytesInlineSeparator: string
    hexBytesInlineEnd: string

    arrayMultilineStart: string
    arrayMultilineSeparator: string
    arrayMultilineEnd: string

    objectMultilineStart: string
    objectMultilineKeySeparator: string
    objectMultilineElementSeparator: string
    objectMultilineEnd: string

    mapMultilineStart: string
    mapMultilineKeySeparator: string
    mapMultilineElementSeparator: string
    mapMultilineEnd: string

    setMultilineStart: string
    setMultilineSeparator: string
    setMultilineEnd: string

    hexBytesMultilineStart: string
    hexBytesMultilineSeparator: string
    hexBytesMultilineEnd: string

    errorName: (name: string) => string
    errorMessage: (message: string) => string
  }
  limits: {
    maxStringLength: number
    maxInlineLength: number
  }
}

export const defaultAnsiTheme: Theme = {
  levelPrefixes: {
    debug: chalk.dim("· "),
    info: chalk.blue.bold("i "),
    warn: chalk.yellow.bold("! "),
    error: chalk.red.bold("‼ "),
  },
  colors: [
    chalk.red,
    chalk.green,
    chalk.yellow,
    chalk.blue,
    chalk.magenta,
    chalk.cyan,
  ] as const,
  values: {
    string: (value) => chalk.dim('"') + chalk.yellow(value) + chalk.dim('"'),
    number: chalk.cyan,
    bigint: (value) => chalk.cyan(value.toString()) + chalk.dim("n"),
    boolean: chalk.magenta,
    symbol: (value) => chalk.magenta(value.toString()),
    function: (type, name) => `${chalk.cyan(`[${type}: ${name}]`)}`,

    undefined: chalk.dim("undefined"),
    null: chalk.dim("null"),
    nan: chalk.dim("NaN"),
    positiveInfinity: chalk.dim("Infinity"),
    negativeInfinity: chalk.dim("-Infinity"),

    date: chalk.green,
    regexp: chalk.red,
    emptyArray: `${chalk.dim("[]")}`,
    emptyObject: `${chalk.dim("{}")}`,
    emptyMap: `Map${chalk.dim("(0) {}")}`,
    emptySet: `Set${chalk.dim("(0) {}")}`,
    weakMap: `WeakMap { ${chalk.cyan("<items unknown>")} }`,
    weakSet: `WeakSet { ${chalk.cyan("<items unknown>")} }`,

    truncatedString: ({ short, original }) =>
      chalk.dim('"') +
      chalk.yellow(short) +
      chalk.dim(`\u2026" (${original.length} chars)`),

    key: chalk.italic,
    symbolKey: (symbol) => `[${chalk.magenta(symbol.toString())}]`,
    quotedKey: (value) => chalk.dim('"') + chalk.yellow(value) + chalk.dim('"'),
    inlineKey: chalk.italic,

    accessor: (type) => chalk.cyan(`[${type}]`),
    anonymousFunctionName: "(anonymous)",
    hexBytes: (hexBytes) => chalk.magenta(hexBytes),
  },
  structures: {
    parameterObjectStart: " ",
    parameterObjectKeySeparator: chalk.italic.dim("="),
    parameterObjectElementSeparator: " ",
    parameterObjectEnd: " ",

    arrayInlineStart: chalk.dim("[ "),
    arrayInlineSeparator: chalk.dim(", "),
    arrayInlineEnd: chalk.dim(" ]"),

    objectConstructor: (name) => name + " ",
    objectConstructorWithSize: (name, size) => name + chalk.dim(`(${size}) `),
    objectInlineStart: chalk.dim("{ "),
    objectInlineKeySeparator: chalk.dim(": "),
    objectInlineElementSeparator: chalk.dim(", "),
    objectInlineEnd: chalk.dim(" }"),

    mapInlineStart: chalk.dim("{ "),
    mapInlineKeySeparator: chalk.dim(" => "),
    mapInlineElementSeparator: chalk.dim(","),
    mapInlineEnd: chalk.dim(" }"),

    setInlineStart: chalk.dim("{ "),
    setInlineSeparator: chalk.dim(", "),
    setInlineEnd: chalk.dim(" }"),

    hexBytesInlineStart: chalk.dim("< "),
    hexBytesInlineSeparator: " ",
    hexBytesInlineEnd: chalk.dim(" >"),

    arrayMultilineStart: chalk.dim("["),
    arrayMultilineSeparator: chalk.dim(","),
    arrayMultilineEnd: chalk.dim("]"),

    objectMultilineStart: chalk.dim("{"),
    objectMultilineKeySeparator: chalk.dim(": "),
    objectMultilineElementSeparator: chalk.dim(","),
    objectMultilineEnd: chalk.dim("}"),

    mapMultilineStart: chalk.dim("{"),
    mapMultilineKeySeparator: chalk.dim(" => "),
    mapMultilineElementSeparator: chalk.dim(","),
    mapMultilineEnd: chalk.dim("}"),

    setMultilineStart: chalk.dim("{"),
    setMultilineSeparator: chalk.dim(","),
    setMultilineEnd: chalk.dim("}"),

    hexBytesMultilineStart: chalk.dim("<"),
    hexBytesMultilineSeparator: chalk.dim(" "),
    hexBytesMultilineEnd: chalk.dim(">"),

    errorName: (name) => chalk.red.bold(`${name}: `),
    errorMessage: (message) => `${chalk.red(message)}`,
  },
  limits: {
    maxStringLength: 100,
    maxInlineLength: 80,
  },
}

const UNQUOTED_KEY_REGEXP = /^[A-Z_a-z]\w*$/

const formatFilePath = (filePath: string) => {
  let protocolPrefix = ""
  if (filePath.startsWith("file://")) {
    protocolPrefix = "file://"
    filePath = filePath.slice(7)
  }

  const match = filePath.match(/^(.*)\/packages\/([\da-z-]+)\/(.*)$/) as
    | [string, string, string, string]
    | null
  if (match) {
    return `${chalk.dim(`${protocolPrefix}${match[1]}/packages/`)}${chalk.cyan(
      match[2]
    )}${chalk.dim("/")}${match[3]}`
  }

  return filePath
}

const colorDeterministically = (
  context: FormattingContext,
  component: string
) => {
  let colored = context.coloringCache.get(component)
  if (!colored) {
    colored = selectBySeed(context.theme.colors, component)(component)
    context.coloringCache.set(component, colored)
  }

  return colored
}

const formatError = (context: FormattingContext, error: Error): string => {
  return error.stack
    ? formatStack(context, error.stack)
    : `${context.theme.structures.errorName(
        `${error.name || "Error"}`
      )}${context.theme.structures.errorMessage(error.message)}`
}

const formatStack = (context: FormattingContext, stack: string): string => {
  const lines = stack.split("\n")

  if (!lines[0]) return stack

  const [name, message] = lines[0]?.split(": ") ?? []

  const formattedCallsites = lines
    .slice(1)
    .map((line) => {
      {
        const match = line.match(/^ {4}at (.*) \((.*):(\d+):(\d+)\)$/) as
          | [string, string, string, string, string]
          | null
        if (match) {
          if (match[2].startsWith("node:")) {
            return chalk.dim(line)
          }
          const isNodeModules = match[2].includes("node_modules")
          const formattedFilePath = isNodeModules
            ? chalk.dim(match[2])
            : formatFilePath(match[2])

          return `    ${chalk.dim("at")} ${
            isNodeModules ? chalk.dim(match[1]) : match[1]
          } ${chalk.dim("(")}${formattedFilePath}${chalk.dim(
            `:${match[3]}:${match[4]})`
          )}`
        }
      }
      {
        const match = line.match(/^ {4}at (.*):(\d+):(\d+)$/) as
          | [string, string, string, string]
          | null
        if (match) {
          const formattedFilePath = formatFilePath(match[1])
          return `    ${chalk.dim("at")} ${formattedFilePath}${chalk.dim(
            `:${match[2]}:${match[3]}`
          )}`
        }
      }

      return chalk.dim(line)
    })
    .filter(Boolean)

  return `${context.theme.structures.errorName(
    name ?? "Error"
  )}${context.theme.structures.errorMessage(message ?? "no message")}${
    formattedCallsites.length > 0 ? `\n${context.indent}` : ""
  }${formattedCallsites.join(`\n${context.indent}`)}`
}

const formatMessage = (context: FormattingContext, message: string) => {
  const firstSpace = message.indexOf(" ")
  const component = message.slice(0, firstSpace === -1 ? 0 : firstSpace)
  const componentColonPosition = component.indexOf(":")

  return componentColonPosition >= 1 &&
    componentColonPosition < component.length - 1
    ? `${colorDeterministically(context, component)} ${message.slice(
        firstSpace + 1
      )}`
    : message
}

interface FormattingContext {
  /**
   * The theme which defines colors and styles to use during formatting.
   */
  theme: Theme

  /**
   * Cache of colored strings to avoid re-coloring the same string multiple times.
   */
  coloringCache: Map<string, string>

  /**
   * A string consisting of tab characters, used to indent nested values.
   */
  indent: string
}

const nearestPowerOfTwoBelow = (value: number) => 1 << (31 - Math.clz32(value))

const formatBinaryData = (
  context: FormattingContext,
  data: Uint8Array | Uint8ClampedArray | Buffer
): string => {
  const hex = [] as string[]

  for (const byte of data) {
    hex.push(byte.toString(16).padStart(2, "0"))
  }

  if (data.byteLength * 3 < context.theme.limits.maxInlineLength) {
    return (
      context.theme.structures.hexBytesInlineStart +
      context.theme.values.hexBytes(
        hex.join(context.theme.structures.hexBytesInlineSeparator)
      ) +
      context.theme.structures.hexBytesInlineEnd
    )
  }

  const output = [] as string[]

  output.push(context.theme.structures.hexBytesMultilineStart)

  const bytesPerLine = nearestPowerOfTwoBelow(
    Math.floor(context.theme.limits.maxInlineLength / 3)
  )
  for (let index = 0; index < hex.length; index += bytesPerLine) {
    output.push(
      `\n${context.indent}` +
        context.theme.values.hexBytes(
          hex
            .slice(index, index + bytesPerLine)
            .join(context.theme.structures.hexBytesMultilineSeparator)
        )
    )
  }

  output.push(
    `\n${context.indent.slice(0, -1)}${
      context.theme.structures.hexBytesMultilineEnd
    }`
  )

  return output.join("")
}

const isArrayInlineable = (
  context: FormattingContext,
  formattedArray: string[]
): boolean => {
  if (formattedArray.some((item) => item.includes("\n"))) return false

  let totalStringWidth = 0

  for (const element of formattedArray) {
    totalStringWidth += stringWidth(element) + 2

    if (totalStringWidth > context.theme.limits.maxInlineLength) return false
  }

  return true
}

const isObjectInlineable = (
  context: FormattingContext,
  formattedObject: readonly (readonly [string, string])[]
): boolean => {
  if (formattedObject.some(([, value]) => value.includes("\n"))) return false

  let totalStringWidth = 0

  for (const [key, value] of formattedObject) {
    totalStringWidth += stringWidth(key) + stringWidth(value) + 4

    if (totalStringWidth > context.theme.limits.maxInlineLength) return false
  }

  return true
}

const formatKey = (
  context: FormattingContext,
  key: string | symbol
): string => {
  if (typeof key === "symbol") {
    return context.theme.values.symbolKey(key)
  }

  if (UNQUOTED_KEY_REGEXP.test(key)) {
    return context.theme.values.key(key)
  }

  return context.theme.values.quotedKey(key)
}

const makeTypedArrayFormatter =
  (constructorName: string) => (context: FormattingContext, value: object) => {
    return (
      context.theme.structures.objectConstructorWithSize(
        constructorName,
        (value as { length: number }).length
      ) + formatArray(context, value as unknown[])
    )
  }

const OBJECT_FORMATTERS: Record<
  string,
  (context: FormattingContext, value: object) => string
> = {
  Date: (context, value) =>
    context.theme.values.date((value as Date).toISOString()),
  RegExp: (context, value) =>
    context.theme.values.regexp((value as RegExp).toString()),
  Error: (context, value) => formatError(context, value as Error),
  Map: (context, value) => {
    const mapValue = value as Map<unknown, unknown>

    const pieces: [string, string][] = []

    for (const [key, value] of mapValue) {
      const formattedKey = formatValue(context, key)
      const formattedValue = formatValue(context, value)

      pieces.push([formattedKey, formattedValue])
    }

    if (mapValue.size === 0) {
      return context.theme.values.emptyMap
    }

    if (isObjectInlineable(context, pieces)) {
      return (
        context.theme.structures.objectConstructorWithSize(
          "Map",
          mapValue.size
        ) +
        context.theme.structures.mapInlineStart +
        pieces
          .map(
            ([key, value]) =>
              `${key}${context.theme.structures.mapInlineKeySeparator}${value}`
          )
          .join(context.theme.structures.mapInlineElementSeparator) +
        context.theme.structures.mapInlineEnd
      )
    }
    return (
      context.theme.structures.objectConstructorWithSize("Map", mapValue.size) +
      context.theme.structures.mapMultilineStart +
      `\n${context.indent}` +
      pieces
        .map(
          ([key, value]) =>
            `${key}${context.theme.structures.mapMultilineKeySeparator}${value}`
        )
        .join(
          `${context.theme.structures.mapMultilineElementSeparator}\n${context.indent}`
        ) +
      `\n${context.indent.slice(0, -1)}` +
      context.theme.structures.mapMultilineEnd
    )
  },
  Set: (context, value) => {
    const setValue = value as Set<unknown>

    const pieces: string[] = []

    for (const element of setValue) {
      pieces.push(formatValue(context, element))
    }

    if (setValue.size === 0) {
      return context.theme.values.emptySet
    }

    if (isArrayInlineable(context, pieces)) {
      return (
        context.theme.structures.objectConstructorWithSize(
          "Set",
          setValue.size
        ) +
        context.theme.structures.setInlineStart +
        pieces.join(context.theme.structures.setInlineSeparator) +
        context.theme.structures.setInlineEnd
      )
    }

    return (
      context.theme.structures.objectConstructorWithSize("Set", setValue.size) +
      context.theme.structures.setMultilineStart +
      `\n${context.indent}` +
      pieces.join(
        `${context.theme.structures.setMultilineSeparator}\n${context.indent}`
      ) +
      `\n${context.indent.slice(0, -1)}` +
      context.theme.structures.setMultilineEnd
    )
  },
  WeakMap: (context) => context.theme.values.weakMap,
  WeakSet: (context) => context.theme.values.weakSet,
  Uint8Array: (context, value) => {
    return `${context.theme.structures.objectConstructorWithSize(
      Buffer.isBuffer(value) ? "Buffer" : "Uint8Array",
      (value as Uint8Array).byteLength
    )}${formatBinaryData(context, value as Uint8Array)}`
  },
  Uint8ClampedArray: (context, value) => {
    return `${context.theme.structures.objectConstructorWithSize(
      "Uint8ClampedArray",
      (value as Uint8ClampedArray).byteLength
    )}${formatBinaryData(context, value as Uint8ClampedArray)}`
  },
  Int8Array: makeTypedArrayFormatter("Int8Array"),
  Int16Array: makeTypedArrayFormatter("Int16Array"),
  Uint16Array: makeTypedArrayFormatter("Uint16Array"),
  Int32Array: makeTypedArrayFormatter("Int32Array"),
  Uint32Array: makeTypedArrayFormatter("Uint32Array"),
  Float32Array: makeTypedArrayFormatter("Float32Array"),
  Float64Array: makeTypedArrayFormatter("Float64Array"),
  BigInt64Array: makeTypedArrayFormatter("BigInt64Array"),
  BigUint64Array: makeTypedArrayFormatter("BigUint64Array"),
}

const formatProperty = (
  context: FormattingContext,
  parent: object,
  key: string | symbol
): string => {
  const descriptor = Object.getOwnPropertyDescriptor(parent, key)!

  if (descriptor.get) {
    if (descriptor.set) {
      return context.theme.values.accessor("Getter/Setter")
    }

    return context.theme.values.accessor("Getter")
  } else if (descriptor.set) {
    return context.theme.values.accessor("Setter")
  }

  return formatValue(context, descriptor.value)
}

const formatArray = (
  context: FormattingContext,
  value: {
    [index: number]: unknown
    length: number
    [Symbol.iterator](): IterableIterator<unknown>
  }
): string => {
  if (value.length === 0) {
    return context.theme.values.emptyArray
  }

  const formattedArray = [] as string[]
  for (const item of value) {
    formattedArray.push(formatValue(context, item))
  }

  if (isArrayInlineable(context, formattedArray)) {
    return (
      context.theme.structures.arrayInlineStart +
      formattedArray.join(context.theme.structures.arrayInlineSeparator) +
      context.theme.structures.arrayInlineEnd
    )
  }

  const output = [] as string[]

  output.push(
    context.theme.structures.arrayMultilineStart,
    "\n",
    context.indent
  )

  let lineLength = 0
  for (const item of formattedArray) {
    const itemLength = stringWidth(item)
    if (lineLength + itemLength > context.theme.limits.maxInlineLength) {
      output.push(
        context.theme.structures.arrayMultilineSeparator,
        "\n",
        context.indent,
        item
      )

      lineLength = itemLength
    } else {
      output.push(
        lineLength === 0
          ? ""
          : context.theme.structures.arrayMultilineSeparator + " ",
        item
      )

      lineLength += itemLength
    }
  }

  output.push(
    "\n",
    context.indent.slice(0, -1),
    context.theme.structures.arrayMultilineEnd
  )

  return output.join("")
}

const formatValue = (context: FormattingContext, value: unknown): string => {
  context.indent += "\t"

  try {
    if (value == null) {
      if (value === undefined) {
        return context.theme.values.undefined
      }

      return context.theme.values.null
    } else if (typeof value === "string") {
      if (value.length > context.theme.limits.maxStringLength) {
        return context.theme.values.truncatedString({
          short: value.slice(0, context.theme.limits.maxStringLength),
          original: value,
        })
      }
      return context.theme.values.string(value)
    } else if (typeof value === "number") {
      if (Number.isNaN(value)) return context.theme.values.nan
      if (value === Number.POSITIVE_INFINITY)
        return context.theme.values.positiveInfinity
      if (value === Number.NEGATIVE_INFINITY)
        return context.theme.values.negativeInfinity

      return context.theme.values.number(value)
    } else if (typeof value === "bigint") {
      return context.theme.values.bigint(value)
    } else if (typeof value === "boolean") {
      return context.theme.values.boolean(value)
    } else if (typeof value === "symbol") {
      return context.theme.values.symbol(value)
    } else if (typeof value === "function") {
      const type = isAsyncFunction(value)
        ? "AsyncFunction"
        : isGeneratorFunction(value)
        ? "GeneratorFunction"
        : "Function"
      const name = value.name || context.theme.values.anonymousFunctionName
      return context.theme.values.function(type, name)
    } else if (Array.isArray(value)) {
      return formatArray(context, value)
    } else if (isObject(value)) {
      const objectConstructor = Object.prototype.toString
        .call(value)
        .slice(8, -1)

      const specialFormatter = OBJECT_FORMATTERS[objectConstructor]
      if (specialFormatter) {
        return specialFormatter(context, value)
      }

      const formattedConstructor =
        objectConstructor === "Object"
          ? ""
          : context.theme.structures.objectConstructor(objectConstructor)

      const keys = [
        ...Object.getOwnPropertyNames(value),
        ...Object.getOwnPropertySymbols(value),
      ]

      if (keys.length === 0) {
        return formattedConstructor + context.theme.values.emptyObject
      }

      const formattedObject = keys.map(
        (key) =>
          [
            formatKey(context, key),
            formatProperty(context, value, key),
          ] as const
      )
      if (isObjectInlineable(context, formattedObject)) {
        return (
          formattedConstructor +
          context.theme.structures.objectInlineStart +
          formattedObject
            .map(
              ([key, value]) =>
                `${key}${context.theme.structures.objectInlineKeySeparator}${value}`
            )
            .join(context.theme.structures.objectInlineElementSeparator) +
          context.theme.structures.objectInlineEnd
        )
      }

      return (
        formattedConstructor +
        context.theme.structures.objectMultilineStart +
        "\n" +
        context.indent +
        formattedObject
          .map(
            ([key, value]) =>
              `${key}${context.theme.structures.objectMultilineKeySeparator}${value}`
          )
          .join(
            `${context.theme.structures.objectMultilineElementSeparator}\n${context.indent}`
          ) +
        `\n${context.indent.slice(0, -1)}` +
        context.theme.structures.objectMultilineEnd
      )
    }
  } finally {
    context.indent = context.indent.slice(0, -1)
  }

  return "[other]"
}

const formatParameters = (
  context: FormattingContext,
  parameters: unknown[]
) => {
  if (parameters.length === 0) return ""

  if (
    parameters.length === 1 &&
    isObject(parameters[0]) &&
    Object.prototype.toString.call(parameters[0]) === "[object Object]" &&
    Object.getOwnPropertySymbols(parameters[0]).length === 0
  ) {
    return (
      context.theme.structures.parameterObjectStart +
      Object.entries(parameters[0] as Record<string, unknown>)
        .map(
          ([key, value]) =>
            `${context.theme.values.inlineKey(key)}${
              context.theme.structures.parameterObjectKeySeparator
            }${formatValue(context, value)}`
        )
        .join(context.theme.structures.parameterObjectElementSeparator) +
      context.theme.structures.parameterObjectEnd
    )
  }

  return parameters
    .map((parameter) => " " + formatValue(context, parameter))
    .join("")
}

const formatEvent = (context: FormattingContext, line: LogMessage) => {
  const levelInsert = context.theme.levelPrefixes[line.type]

  let error: Error | undefined
  if (isObject(line.parameters[0]) && isError(line.parameters[0]["error"])) {
    error = line.parameters[0]["error"]
    // eslint-disable-next-line @typescript-eslint/no-dynamic-delete
    delete line.parameters[0]["error"]

    if (
      line.parameters.length === 1 &&
      Object.keys(line.parameters[0]).length === 0
    ) {
      line.parameters = []
    }
  }

  return `${levelInsert}${formatMessage(
    context,
    line.message
  )}${formatParameters(context, line.parameters)}\n${
    error ? "\n" + formatError(context, error) + "\n\n" : ""
  }`
}

export interface CliFormatterOptions {
  theme?: Theme | undefined
}

export const createCliFormatter = ({
  theme = defaultAnsiTheme,
}: CliFormatterOptions = {}): LogEventFormatter => {
  const coloringCache = new Map<string, string>()
  const log: LogEventFormatter = (line) => {
    if (line.type === "clear") {
      return ansiEscapes.clearScreen
    }

    const context = {
      theme,
      coloringCache,
      indent: "",
    }

    return formatEvent(context, line)
  }

  return log
}
