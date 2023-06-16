export interface IndentStringOptions {
  /**
   * Whether to indent lines that only contain whitespace.
   */
  indentEmptyLines?: boolean

  /**
   * Whether to indent the first line.
   */
  indentFirstLine?: boolean

  /**
   * A string that will be repeated `count` times and prepended to each line.
   */
  indentUnit?: string
}

const REGEX_NON_EMPTY_OR_FIRST_LINES = /(?<=\n)^(?!\s*$)/gm
const REGEX_NON_EMPTY_LINES = /^(?!\s*$)/gm
const REGEX_NON_FIRST_LINES = /(?<=\n)^/g
const REGEX_ALL_LINES = /^/gm

/**
 * Indent each line in a string.
 *
 * Based on the indent-string package by Sindre Sorhus. See LICENSE.indent-string.
 *
 * @param text - Target string.
 * @param count - Number of times to repeat `indentUnit`.
 * @param options - Options.
 * @returns The indented string.
 */
export const indentString = (
  text: string,
  count = 1,
  {
    indentEmptyLines = false,
    indentFirstLine = true,
    indentUnit = " ",
  }: IndentStringOptions = {}
) => {
  if (typeof text !== "string") {
    throw new TypeError('"text" is not a string')
  }

  if (typeof indentUnit !== "string") {
    throw new TypeError('"indent" is not a string')
  }

  if (count === 0 || indentUnit === "") {
    return text
  }

  const regex = indentEmptyLines
    ? indentFirstLine
      ? REGEX_ALL_LINES
      : REGEX_NON_FIRST_LINES
    : indentFirstLine
    ? REGEX_NON_EMPTY_LINES
    : REGEX_NON_EMPTY_OR_FIRST_LINES

  return text.replace(regex, indentUnit.repeat(count))
}
