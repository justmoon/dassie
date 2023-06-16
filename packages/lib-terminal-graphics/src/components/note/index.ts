// eslint-disable-next-line unicorn/import-style
import chalk from "chalk"
import wrapAnsi from "wrap-ansi"

import { indentString } from "../../helpers/indent-string"
import { maybeUnicode } from "../../helpers/unicode-fallback"
import { StepStyle } from "../../theme"
import { StaticTerminalComponent } from "../../types/terminal-component"

export interface NoteOptions {
  style?: StepStyle
  title: string
  body?: string
}

export const note = ({ title, body, style = "info" }: NoteOptions) =>
  ({
    type: "static",
    render: ({ columns, theme }) => [
      chalk[theme.stepStyles[style].color].inverse.bold(
        ` ${maybeUnicode(theme.stepStyles[style].icon)} `
      ),
      chalk.bold(
        ` ${indentString(wrapAnsi(title, columns - 4), 4, {
          indentFirstLine: false,
        })} `
      ),
      "\n",
      body ? chalk.dim(indentString(wrapAnsi(body, columns - 4), 4)) : "",
      body ? "\n" : "",
    ],
  } satisfies StaticTerminalComponent)
