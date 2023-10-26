import chalk from "chalk"
import wrapAnsi from "wrap-ansi"

import { indentString } from "../../helpers/indent-string"
import { maybeUnicode } from "../../helpers/unicode-fallback"
import { PromptTheme } from "../../theme"
import { InteractiveTerminalComponent } from "../../types/terminal-component"
import { cancel } from "../common/actions/finalizations"

export interface ConfirmOptions {
  title: string
  initialValue?: boolean
  paddingTop?: number
  paddingBottom?: number
}

export interface ConfirmState {
  state: "normal" | "confirm" | "cancel"
  value: boolean
}

const renderOptions = (
  state: ConfirmState["state"],
  value: boolean,
  theme: PromptTheme,
) => {
  switch (state) {
    case "normal": {
      return value
        ? [
            chalk.green(maybeUnicode(theme.radio.checked)),
            " ",
            chalk.underline("Y"),
            "es",
            chalk.dim(" / "),
            chalk.dim(maybeUnicode(theme.radio.unchecked)),
            " ",
            chalk.dim.underline("N"),
            chalk.dim("o"),
          ]
        : [
            chalk.dim(maybeUnicode(theme.radio.unchecked)),
            " ",
            chalk.dim.underline("Y"),
            chalk.dim("es"),
            chalk.dim(" / "),
            chalk.red(maybeUnicode(theme.radio.checked)),
            " ",
            chalk.underline("N"),
            "o",
          ]
    }

    case "confirm": {
      return [chalk.dim(value ? "Yes" : "No")]
    }

    case "cancel": {
      return [chalk.dim.strikethrough(value ? "Yes" : "No")]
    }
  }
}

export const confirm = ({
  title,
  initialValue = false,
  paddingTop = 1,
  paddingBottom = 0,
}: ConfirmOptions) =>
  ({
    type: "interactive",
    initialState: {
      state: "normal",
      value: initialValue,
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
          case "y": {
            return {
              ...state,
              state: "confirm",
              value: true,
            }
          }

          case "n": {
            return {
              ...state,
              state: "confirm",
              value: false,
            }
          }

          case "left":
          case "right": {
            return {
              ...state,
              value: !state.value,
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
    render: ({ state, value }, { columns, theme }) => [
      "\n".repeat(paddingTop),
      chalk.blueBright.inverse.bold(` ? `),
      chalk.bold(
        ` ${indentString(wrapAnsi(title, columns - 4), 4, {
          indentFirstLine: false,
        })} `,
      ),
      "\n",
      " ".repeat(4),
      ...renderOptions(state, value, theme),
      "\n".repeat(1 + paddingBottom),
    ],
    result: ({ value }) => value,
  }) satisfies InteractiveTerminalComponent<ConfirmState, boolean>
