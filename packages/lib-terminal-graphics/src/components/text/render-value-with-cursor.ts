import chalk from "chalk"

import { rightGrapheme } from "../../helpers/string-offsets"

export const renderValueWithCursor = (
  value: string,
  cursor: number
): string[] => {
  if (cursor >= value.length) {
    return [value, chalk.inverse(" ")]
  }

  const cursorEnd = rightGrapheme(value, cursor)
  return [
    value.slice(0, cursor),
    chalk.inverse(value.slice(cursor, cursorEnd)),
    value.slice(cursorEnd),
  ]
}
