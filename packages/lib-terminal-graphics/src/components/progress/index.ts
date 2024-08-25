import chalk from "chalk"
import wrapAnsi from "wrap-ansi"

import { createStore } from "@dassie/lib-reactive"

import { indentString } from "../../helpers/indent-string"
import {
  generateDeterminateProgressBar,
  generateIndeterminateProgressBar,
  getTick,
} from "../../helpers/progress-bar"
import { maybeUnicode } from "../../helpers/unicode-fallback"
import { StepStyle } from "../../theme"
import { type DynamicTerminalComponent } from "../../types/terminal-component"

export interface ProgressOptions {
  style?: StepStyle
  description: string
  paddingTop?: number
  paddingBottom?: number
  refreshInterval?: number
}

export interface ProgressState {
  progress: number | undefined
}

export const progress = ({
  description,
  style = "info",
  paddingTop = 1,
  paddingBottom = 0,
  refreshInterval = 80,
}: ProgressOptions) => {
  const store = createStore({ progress: undefined } as ProgressState).actions({
    setProgress: (progress: number) => (state) => ({ ...state, progress }),
  })

  return {
    type: "dynamic",
    state: store,
    refreshInterval,
    render: ({ progress }, isFinal, { theme, columns }) => [
      "\n".repeat(paddingTop),
      isFinal ?
        chalk.bold[theme.stepStyles.success.color](
          maybeUnicode(theme.stepStyles.success.icon),
        )
      : maybeUnicode(
          theme.spinner[
            Math.floor(Date.now() / refreshInterval) % theme.spinner.length
          ]!,
        ),

      " ",
      chalk.dim(
        indentString(wrapAnsi(description, columns - 4), 4, {
          indentFirstLine: false,
        }),
      ),

      ...(isFinal ?
        []
      : [
          "\n",
          "\n",
          progress === undefined ?
            chalk[theme.stepStyles[style].color].bgGray(
              generateIndeterminateProgressBar(
                getTick(Date.now(), refreshInterval),
                columns,
              ),
            )
          : chalk[theme.stepStyles[style].color].bgGray(
              generateDeterminateProgressBar(
                getTick(Date.now(), refreshInterval),
                progress,
                columns,
              ),
            ),
          "\n",
        ]),
      "\n".repeat(1 + paddingBottom),
    ],
  } satisfies DynamicTerminalComponent<ProgressState>
}
