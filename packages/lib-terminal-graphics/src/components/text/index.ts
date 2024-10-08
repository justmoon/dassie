import chalk from "chalk"
import wrapAnsi from "wrap-ansi"

import { indentString } from "../../helpers/indent-string"
import type { InteractiveTerminalComponent } from "../../types/terminal-component"
import { cancel, submit } from "../common/actions/finalizations"
import {
  cursorEnd,
  cursorHome,
  cursorLeft,
  cursorRight,
  cursorWordLeft,
  cursorWordRight,
} from "./actions/cursor-motions"
import {
  deleteLeft,
  deleteLineLeft,
  deleteLineRight,
  deleteRight,
  deleteWordLeft,
  deleteWordRight,
  insertString,
} from "./actions/text-operations"
import { renderValueWithCursor } from "./render-value-with-cursor"

export interface TextOptions {
  title: string
  explanation?: string
  initialValue?: string
  paddingTop?: number
  paddingBottom?: number
}

export interface TextState {
  state: "normal" | "confirm" | "cancel"
  value: string
  cursor: number
}

export const text = ({
  title,
  explanation,
  initialValue,
  paddingTop = 1,
  paddingBottom = 0,
}: TextOptions) =>
  ({
    type: "interactive",
    initialState: {
      state: "normal",
      value: initialValue ?? "",
      cursor: 0,
    },
    isFinal: (state) => state.state === "confirm" || state.state === "cancel",
    update: (state, key) => {
      if (key.ctrl) {
        switch (key.name) {
          case "c": {
            return cancel(state)
          }

          case "h": {
            return deleteLeft(state)
          }

          case "d": {
            return deleteRight(state)
          }

          case "u": {
            return deleteLineLeft(state)
          }

          case "k": {
            return deleteLineRight(state)
          }

          case "a": {
            return cursorHome(state)
          }

          case "e": {
            return cursorEnd(state)
          }

          case "b": {
            return cursorLeft(state)
          }

          case "f": {
            return cursorRight(state)
          }

          case "w":
          case "backspace": {
            return deleteWordLeft(state)
          }
        }
      } else if (key.meta) {
        switch (key.name) {
          case "b": {
            return cursorWordLeft(state)
          }

          case "f": {
            return cursorWordRight(state)
          }

          case "d":
          case "delete": {
            return deleteWordRight(state)
          }

          case "backspace": {
            return deleteWordLeft(state)
          }
        }
      } else {
        switch (key.name) {
          case "enter":
          case "return": {
            return submit(state)
          }

          case "backspace": {
            return deleteLeft(state)
          }

          case "delete": {
            return deleteRight(state)
          }

          case "left": {
            return cursorLeft(state)
          }

          case "right": {
            return cursorRight(state)
          }

          case "home": {
            return cursorHome(state)
          }

          case "end": {
            return cursorEnd(state)
          }

          default: {
            if (
              typeof key.sequence === "string" &&
              key.sequence.length > 0 &&
              key.sequence.codePointAt(0)! > 0x1f
            ) {
              return insertString(key.sequence)(state)
            }
          }
        }
      }

      return state
    },
    render: ({ state, value, cursor }, { columns }) => [
      "\n".repeat(paddingTop),
      chalk.blueBright.inverse.bold(` ? `),
      chalk.bold(
        ` ${indentString(wrapAnsi(title, columns - 4), 4, {
          indentFirstLine: false,
        })} `,
      ),
      "\n",
      explanation && state === "normal" ?
        chalk.dim(indentString(wrapAnsi(explanation, columns - 4), 4))
      : "",
      explanation && state === "normal" ? "\n" : "",
      ...(state === "confirm" ? [indentString(chalk.dim(value), 4)]
      : state === "cancel" ?
        [indentString(chalk.dim.strikethrough(value || "(canceled)"), 4)]
      : [
          "\n  ",
          chalk.dim.green(">"),
          " ",
          renderValueWithCursor(value, cursor).join(""),
        ]),
      "\n".repeat(1 + paddingBottom),
    ],
    result: ({ value }) => value,
  }) satisfies InteractiveTerminalComponent<TextState, string>
