// eslint-disable-next-line unicorn/import-style
import chalk from "chalk"
import { enableMapSet, produce } from "immer"
import wrapAnsi from "wrap-ansi"

import { createStore } from "@dassie/lib-reactive"

import { indentString } from "../../helpers/indent-string"
import { generateIndeterminateProgressBar } from "../../helpers/progress-bar"
import { maybeUnicode } from "../../helpers/unicode-fallback"
import { StepStyle } from "../../theme"
import { type DynamicTerminalComponent } from "../../types/terminal-component"

enableMapSet()

export interface TasklistOptions {
  style?: StepStyle
  paddingTop?: number
  paddingBottom?: number
  refreshInterval?: number
}

export interface TaskState {
  description: string
  progress: number | "indeterminate" | "done" | "error"
}

export interface TasklistState {
  tasks: Map<string, TaskState>
}

export const tasklist = ({
  style = "info",
  paddingTop = 1,
  paddingBottom = 0,
  refreshInterval = 80,
}: TasklistOptions) => {
  const store = createStore({ tasks: new Map() } as TasklistState, {
    addTask: (taskId: string, initialState: TaskState) =>
      produce((draft) => {
        draft.tasks.set(taskId, initialState)
      }),
    updateTask: (
      taskId: string,
      update: Partial<TaskState> | ((state: TaskState) => TaskState),
    ) =>
      produce((draft) => {
        const task = draft.tasks.get(taskId)
        if (task) {
          draft.tasks.set(
            taskId,
            typeof update === "function"
              ? update(task)
              : { ...task, ...update },
          )
        }
      }),
  })

  return {
    type: "dynamic",
    state: store,
    refreshInterval,
    render: ({ tasks }, isFinal, { theme, columns }) => [
      "\n".repeat(paddingTop),
      ...[...tasks.values()].flatMap(({ description, progress }) => [
        " ",
        progress === "done"
          ? chalk.bold[theme.stepStyles.success.color](
              maybeUnicode(theme.stepStyles.success.icon),
            )
          : progress === "error"
            ? chalk.bold[theme.stepStyles.error.color](
                maybeUnicode(theme.stepStyles.error.icon),
              )
            : maybeUnicode(
                theme.spinner[
                  Math.floor(Date.now() / refreshInterval) %
                    theme.spinner.length
                ]!,
              ),

        " ",
        chalk.dim(
          indentString(wrapAnsi(description, columns - 4), 4, {
            indentFirstLine: false,
          }),
        ),
        "\n",
      ]),

      ...(isFinal
        ? []
        : [
            "\n",
            chalk[theme.stepStyles[style].color].bgGray(
              generateIndeterminateProgressBar(
                Math.floor(Date.now() / refreshInterval),
                columns,
              ),
            ),
            "\n",
          ]),
      "\n".repeat(1 + paddingBottom),
    ],
  } satisfies DynamicTerminalComponent<TasklistState>
}
