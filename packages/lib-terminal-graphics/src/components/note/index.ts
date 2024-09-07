import chalk from "chalk"
import wrapAnsi from "wrap-ansi"

import { indentString } from "../../helpers/indent-string"
import { maybeUnicode } from "../../helpers/unicode-fallback"
import type { StepStyle } from "../../theme"
import type { StaticTerminalComponent } from "../../types/terminal-component"

export interface NoteOptions {
  style?: StepStyle
  title: string
  body?: string
  paddingTop?: number
  paddingBottom?: number
}

export const note = ({
  title,
  body,
  style = "info",
  paddingTop = 1,
  paddingBottom = 0,
}: NoteOptions) =>
  ({
    type: "static",
    render: ({ columns, theme }) => [
      "\n".repeat(paddingTop),
      chalk[theme.stepStyles[style].color].inverse.bold(
        ` ${maybeUnicode(theme.stepStyles[style].icon)} `,
      ),
      chalk.bold(
        ` ${indentString(wrapAnsi(title, columns - 4), 4, {
          indentFirstLine: false,
        })} `,
      ),
      body ? "\n" : "",
      body ? chalk.dim(indentString(wrapAnsi(body, columns - 4), 4)) : "",
      "\n".repeat(1 + paddingBottom),
    ],
  }) satisfies StaticTerminalComponent
