import stripAnsi from "strip-ansi"
import wcwidth from "wcwidth"

export const countLines = (text: string, columns: number) => {
  let lineCount = 0
  for (const line of stripAnsi(text).split("\n")) {
    lineCount += Math.max(1, Math.ceil(wcwidth(line) / columns))
  }
  return lineCount
}
