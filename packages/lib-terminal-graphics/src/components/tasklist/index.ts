import chalk from "chalk"
import { enableMapSet, produce } from "immer"
import stringWidth from "string-width"
import wrapAnsi from "wrap-ansi"

import { createStore } from "@dassie/lib-reactive"

import { indentString } from "../../helpers/indent-string"
import {
  generateDeterminateProgressBar,
  generateIndeterminateProgressBar,
} from "../../helpers/progress-bar"
import { maybeUnicode } from "../../helpers/unicode-fallback"
import { StepStyle } from "../../theme"
import { type DynamicTerminalComponent } from "../../types/terminal-component"

enableMapSet()

export interface TasklistOptions {
  style?: StepStyle
  paddingTop?: number
  paddingBetweenTasks?: number
  paddingBottom?: number
  refreshInterval?: number
  maxDescriptionWidth?: number
}

export interface TaskState {
  description: string
  progress: number | "indeterminate" | "done" | "error"
}

export interface TasklistState {
  tasks: Map<string, TaskState>
}

const DEFAULT_MAX_DESCRIPTION_WIDTH = 40

/**
 * Number of columns taken up by the bullet and related whitespace.
 */
const SPACE_FOR_BULLET = 4

export const tasklist = ({
  style = "info",
  paddingTop = 1,
  paddingBetweenTasks = 0,
  paddingBottom = 1,
  refreshInterval = 80,
  maxDescriptionWidth = DEFAULT_MAX_DESCRIPTION_WIDTH,
}: TasklistOptions) => {
  const store = createStore({ tasks: new Map() } as TasklistState).actions({
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
            typeof update === "function" ?
              update(task)
            : { ...task, ...update },
          )
        }
      }),
  })

  return {
    type: "dynamic",
    state: store,
    refreshInterval,
    render: ({ tasks }, isFinal, { theme, columns }) => {
      return [
        "\n".repeat(paddingTop),
        ...[...tasks.values()].flatMap(({ description, progress }) => {
          const descriptionWidth = stringWidth(description)
          const progressBarWidth =
            descriptionWidth > maxDescriptionWidth ? 0 : (
              Math.max(0, columns - SPACE_FOR_BULLET - maxDescriptionWidth - 1)
            )
          const descriptionPadding =
            Math.max(0, maxDescriptionWidth - descriptionWidth) + 1
          return [
            " ",
            progress === "done" ?
              chalk.bold[theme.stepStyles.success.color](
                maybeUnicode(theme.stepStyles.success.icon),
              )
            : progress === "error" ?
              chalk.bold[theme.stepStyles.error.color](
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

            ...((
              isFinal ||
              !progressBarWidth ||
              progress === "done" ||
              progress === "error"
            ) ?
              []
            : [
                " ".repeat(descriptionPadding),
                chalk[theme.stepStyles[style].color].bgGray(
                  progress === "indeterminate" ?
                    generateIndeterminateProgressBar(
                      Math.floor(Date.now() / refreshInterval),
                      progressBarWidth,
                    )
                  : generateDeterminateProgressBar(
                      Math.floor(Date.now() / refreshInterval),
                      progress,
                      progressBarWidth,
                    ),
                ),
              ]),
            "\n".repeat(1 + paddingBetweenTasks),
          ]
        }),
        "\n".repeat(paddingBottom),
      ]
    },
  } satisfies DynamicTerminalComponent<TasklistState>
}
