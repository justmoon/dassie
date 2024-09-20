import chalk from "chalk"
import wrapAnsi from "wrap-ansi"

import { indentString } from "../../helpers/indent-string"
import { maybeUnicode } from "../../helpers/unicode-fallback"
import type { PromptTheme } from "../../theme"
import type { InteractiveTerminalComponent } from "../../types/terminal-component"
import { cancel } from "../common/actions/finalizations"

export type SelectChoice = {
  value: string
  label?: string | undefined
  description?: string | undefined
}

export interface SelectOptions<TOptions extends SelectChoice[]> {
  title: string
  choices: TOptions
  initialValue?: TOptions[number]["value"] | undefined
  paddingTop?: number
  paddingBottom?: number
}

export interface SelectState<TOptions extends SelectChoice[]> {
  state: "normal" | "confirm" | "cancel"
  choices: TOptions
  value: number
}

const renderOptions = <TOptions extends SelectChoice[]>(
  state: SelectState<TOptions>,
  theme: PromptTheme,
  columns: number,
) => {
  const selectedOption = state.choices.find((_, index) => index === state.value)
  switch (state.state) {
    case "normal": {
      return state.choices.flatMap(
        ({ value, label = value, description }, index) => [
          "\n",
          "  ",
          index === state.value ?
            chalk.green(maybeUnicode(theme.radio.checked))
          : chalk.dim(maybeUnicode(theme.radio.unchecked)),
          " ",
          index === state.value ? chalk.bold(label) : chalk.dim.bold(label),
          ...(description == undefined ?
            []
          : [
              "\n",
              chalk.dim(indentString(wrapAnsi(description, columns - 4), 4)),
            ]),
          "\n",
        ],
      )
    }

    case "confirm": {
      return [
        " ".repeat(4),
        chalk.dim(selectedOption?.label ?? selectedOption?.value ?? "???"),
        "\n",
      ]
    }

    case "cancel": {
      return [
        " ".repeat(4),
        chalk.dim.strikethrough(
          selectedOption?.label ?? selectedOption?.value ?? "???",
        ),
        "\n",
      ]
    }
  }
}

export const select = <TOptions extends SelectChoice[]>({
  title,
  choices,
  initialValue,
  paddingTop = 1,
  paddingBottom = 0,
}: SelectOptions<TOptions>) =>
  ({
    type: "interactive",
    initialState: {
      state: "normal",
      choices,
      value:
        initialValue == undefined ? 0 : (
          Math.max(
            choices.findIndex((choice) => choice.value === initialValue),
            0,
          )
        ),
    },
    isFinal: (state) => state.state === "confirm" || state.state === "cancel",
    update: (state, key) => {
      if (key.ctrl) {
        switch (key.name) {
          case "c": {
            return cancel(state)
          }
        }
      } else if (key.meta) {
        // no bindings
      } else {
        switch (key.name) {
          case "left":
          case "up": {
            return {
              ...state,
              value: Math.max(0, state.value - 1),
            }
          }

          case "right":
          case "down": {
            return {
              ...state,
              value: Math.min(state.choices.length - 1, state.value + 1),
            }
          }

          case "return":
          case "enter": {
            return {
              ...state,
              state: "confirm",
            }
          }
        }
      }

      return state
    },
    render: (state, { columns, theme }) => [
      "\n".repeat(paddingTop),
      chalk.blueBright.inverse.bold(` ? `),
      chalk.bold(
        ` ${indentString(wrapAnsi(title, columns - 4), 4, {
          indentFirstLine: false,
        })} `,
      ),
      "\n",
      ...renderOptions(state, theme, columns),
      "\n".repeat(paddingBottom),
    ],
    result: ({ value }) => choices.find((_, index) => index === value)?.value,
  }) satisfies InteractiveTerminalComponent<
    SelectState<TOptions>,
    TOptions[number]["value"] | undefined
  >
