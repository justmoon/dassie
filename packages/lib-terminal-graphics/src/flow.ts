import cursor from "cli-cursor"

import process, { stdin, stdout } from "node:process"
import { ReadStream, WriteStream } from "node:tty"

import { Canceled } from "./canceled"
import { countLines } from "./functions/count-lines"
import { createInputReader } from "./input-reader"
import { DEFAULT_THEME, PromptTheme } from "./theme"
import {
  InferComponentResult,
  InteractiveTerminalComponent,
  StaticTerminalComponent,
} from "./types/terminal-component"

export interface FlowOptions {
  inputStream?: ReadStream
  outputStream?: WriteStream
  theme?: PromptTheme
}

export const createFlow = ({
  inputStream = stdin,
  outputStream = stdout,
  theme = DEFAULT_THEME,
}: FlowOptions = {}) => {
  let previousOutput: string | undefined
  const render = (chunks: readonly string[]) => {
    if (previousOutput && outputStream.isTTY) {
      outputStream.cursorTo(0)
      const previousLines = countLines(previousOutput, outputStream.columns)
      for (let index = 0; index < previousLines; index++) {
        if (index > 0) {
          outputStream.moveCursor(0, -1)
        }
        outputStream.clearLine(0)
      }
    }

    const concatenatedOutput = chunks.join("")
    outputStream.write(concatenatedOutput)

    previousOutput = concatenatedOutput
  }

  const getRenderEnvironment = () => ({
    columns: outputStream.columns,
    theme,
  })

  return {
    show(component: StaticTerminalComponent): void {
      render(component.render(getRenderEnvironment()))
      previousOutput = undefined
    },

    async interact<TComponent extends InteractiveTerminalComponent>(
      component: TComponent
    ): Promise<InferComponentResult<TComponent> | Canceled> {
      let state = component.initialState

      render(component.render(state, getRenderEnvironment()))

      if (component.isFinal(state)) {
        return component.result(state) as InferComponentResult<TComponent>
      }

      cursor.hide(outputStream)

      try {
        const reader = createInputReader({ inputStream })
        for await (const key of reader.keys()) {
          if (key.ctrl && key.name === "z") {
            if (process.platform === "win32") continue

            process.once("SIGCONT", () => {
              cursor.hide(outputStream)
              inputStream.setRawMode(true)
              inputStream.resume()
              render(component.render(state, getRenderEnvironment()))
            })

            inputStream.setRawMode(false)
            cursor.show(outputStream)

            // TODO: There is a bug here where the user has to press Ctrl+Z again because the following line doesn't work.
            process.kill(process.pid, "SIGTSTP")
            continue
          }

          state = component.update(state, key)
          render(component.render(state, getRenderEnvironment()))

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

      previousOutput = undefined
      return component.result(state) as InferComponentResult<TComponent>
    },
  }
}
