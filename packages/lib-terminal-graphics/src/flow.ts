import cursor from "cli-cursor"

import process, { stdin, stdout } from "node:process"
import { ReadStream, WriteStream } from "node:tty"

import { createScope } from "@dassie/lib-reactive"

import { Canceled } from "./canceled"
import {
  CLEAR_REST_OF_SCREEN,
  CLEAR_TO_END_OF_LINE,
} from "./constants/ansi-codes"
import { countLines } from "./functions/count-lines"
import { commonPrefixLines } from "./helpers/common-prefix-lines"
import { createInputReader } from "./input-reader"
import { DEFAULT_THEME, PromptTheme } from "./theme"
import {
  type DynamicTerminalComponent,
  InferComponentResult,
  InteractiveTerminalComponent,
  StaticTerminalComponent,
} from "./types/terminal-component"

export interface FlowOptions {
  inputStream?: ReadStream
  outputStream?: WriteStream
  theme?: PromptTheme
}

export type Flow = ReturnType<typeof createFlow>

interface ActiveComponent {
  clear(): string | undefined
  render(): string
}

export const createFlow = ({
  inputStream = stdin,
  outputStream = stdout,
  theme = DEFAULT_THEME,
}: FlowOptions = {}) => {
  let activeComponent: ActiveComponent | undefined

  const replace = (
    previousOutput: string | undefined,
    newOutput: string,
  ): void => {
    if (!outputStream.isTTY || previousOutput === undefined) {
      outputStream.write(newOutput)
      return
    }

    const commonLineIndex = commonPrefixLines(previousOutput, newOutput)

    previousOutput = previousOutput.slice(commonLineIndex)
    newOutput = newOutput.slice(commonLineIndex)

    const previousLineCount = countLines(previousOutput, outputStream.columns)

    outputStream.cursorTo(0)
    outputStream.moveCursor(0, -previousLineCount + 1)

    const outputLines = newOutput.split("\n")
    for (const [index, line] of outputLines.entries()) {
      outputStream.write(line + CLEAR_TO_END_OF_LINE)

      if (index < outputLines.length - 1) outputStream.write("\n")
    }
    outputStream.write(CLEAR_REST_OF_SCREEN)
  }

  const getRenderEnvironment = () => ({
    columns: outputStream.columns,
    theme,
  })

  function show(component: StaticTerminalComponent | string): void {
    const previousOutput = activeComponent?.clear()

    const newOutput =
      typeof component === "string" ? component : (
        component.render(getRenderEnvironment()).join("") +
        (activeComponent?.render() ?? "")
      )

    replace(previousOutput, newOutput)
  }

  return {
    show,

    async attach<TComponent extends DynamicTerminalComponent>(
      component: TComponent,
      activity: (state: TComponent["state"]) => Promise<void>,
    ): Promise<void> {
      if (activeComponent) {
        throw new Error(
          "Cannot render another interactive component until the previous one is finished",
        )
      }

      let isFinal = false
      let previousOutput: string | undefined

      function renderComponent() {
        return (previousOutput = component
          .render(component.state.read(), isFinal, getRenderEnvironment())
          .join(""))
      }

      function clearComponent() {
        const previousOutputCache = previousOutput
        previousOutput = undefined
        return previousOutputCache
      }

      activeComponent = {
        render: renderComponent,
        clear: clearComponent,
      }

      function update() {
        replace(clearComponent(), renderComponent())
      }

      const scope = createScope("dynamic step")

      component.state.values.on(scope, update)

      const interval =
        component.refreshInterval > 0 ?
          setInterval(update, component.refreshInterval)
        : undefined

      update()
      await activity(component.state)

      await scope.dispose()

      if (interval) clearInterval(interval)

      isFinal = true
      update()

      cursor.show(outputStream)
      activeComponent = undefined
    },

    async interact<TComponent extends InteractiveTerminalComponent>(
      component: TComponent,
    ): Promise<InferComponentResult<TComponent> | Canceled> {
      if (activeComponent) {
        throw new Error(
          "Cannot render another interactive component until the previous one is finished",
        )
      }

      let state = component.initialState
      let previousOutput: string | undefined

      if (component.isFinal(state)) {
        return component.result(state) as InferComponentResult<TComponent>
      }

      function renderComponent() {
        return (previousOutput = component
          .render(state, getRenderEnvironment())
          .join(""))
      }

      function clearComponent() {
        const previousOutputCache = previousOutput
        previousOutput = undefined
        return previousOutputCache
      }

      activeComponent = {
        render: renderComponent,
        clear: clearComponent,
      }

      function update() {
        replace(clearComponent(), renderComponent())
      }

      cursor.hide(outputStream)
      update()

      try {
        const reader = createInputReader({ inputStream })
        for await (const key of reader.keys()) {
          if (key.ctrl && key.name === "z") {
            if (process.platform === "win32") continue

            process.once("SIGCONT", () => {
              cursor.hide(outputStream)
              inputStream.setRawMode(true)
              inputStream.resume()
              replace(undefined, renderComponent())
            })

            inputStream.setRawMode(false)
            cursor.show(outputStream)

            // TODO: There is a bug here where the user has to press Ctrl+Z again because the following line doesn't work.
            process.kill(process.pid, "SIGTSTP")
            continue
          }

          state = component.update(state, key)
          update()

          // We handle the cancel key after we have sent it to the component in
          // case the component wants to change its state in response to it.
          if (key.ctrl && key.name === "c") {
            previousOutput = undefined
            return Canceled
          }

          if (component.isFinal(state)) break
        }
      } finally {
        cursor.show(outputStream)
      }

      activeComponent = undefined
      return component.result(state) as InferComponentResult<TComponent>
    },

    emptyLine(count = 1) {
      show("\n".repeat(count))
    },
  }
}
