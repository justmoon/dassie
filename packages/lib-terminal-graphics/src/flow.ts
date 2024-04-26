import cursor from "cli-cursor"

import process, { stdin, stdout } from "node:process"
import { ReadStream, WriteStream } from "node:tty"

import { createLifecycleScope } from "@dassie/lib-reactive"

import { Canceled } from "./canceled"
import { countLines } from "./functions/count-lines"
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
  render: () => void
  clear: () => void
}

export const createFlow = ({
  inputStream = stdin,
  outputStream = stdout,
  theme = DEFAULT_THEME,
}: FlowOptions = {}) => {
  let activeComponent: ActiveComponent | undefined

  const clear = (previousOutput: string) => {
    if (outputStream.isTTY) {
      outputStream.cursorTo(0)
      const previousLines = countLines(previousOutput, outputStream.columns)
      for (let index = 0; index < previousLines; index++) {
        if (index > 0) {
          outputStream.moveCursor(0, -1)
        }
        outputStream.clearLine(0)
      }
    }
  }

  const render = (chunks: readonly string[]) => {
    const concatenatedOutput = chunks.join("")
    outputStream.write(concatenatedOutput)

    return concatenatedOutput
  }

  const getRenderEnvironment = () => ({
    columns: outputStream.columns,
    theme,
  })

  return {
    show(component: StaticTerminalComponent | string): void {
      activeComponent?.clear()

      if (typeof component === "string") {
        render([component])
        return
      }

      render(component.render(getRenderEnvironment()))
      activeComponent?.render()
    },

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
        previousOutput = render(
          component.render(
            component.state.read(),
            isFinal,
            getRenderEnvironment(),
          ),
        )
      }

      function clearComponent() {
        if (previousOutput) clear(previousOutput)
        previousOutput = undefined
      }

      activeComponent = {
        render: renderComponent,
        clear: clearComponent,
      }

      function update() {
        clearComponent()
        renderComponent()
      }

      const lifecycle = createLifecycleScope("dynamic step")

      component.state.on({ lifecycle }, update)

      const interval =
        component.refreshInterval > 0
          ? setInterval(update, component.refreshInterval)
          : undefined

      update()
      await activity(component.state)

      await lifecycle.dispose()

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
        previousOutput = render(component.render(state, getRenderEnvironment()))
      }

      function clearComponent() {
        if (previousOutput) clear(previousOutput)
      }

      activeComponent = {
        render: renderComponent,
        clear: clearComponent,
      }

      function update() {
        clearComponent()
        renderComponent()
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

    emptyLine(count: number = 1) {
      activeComponent?.clear()

      render(["\n".repeat(count)])

      activeComponent?.render()
    },
  }
}
