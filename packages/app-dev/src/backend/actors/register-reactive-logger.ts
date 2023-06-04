import chalk from "chalk"

import {
  SerializableLogEvent,
  captureConsole,
  createCliFormatter,
  createJsonFormatter,
  selectBySeed,
} from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

import { logsStore } from "../../common/stores/logs"
import { COLORS } from "../../frontend/constants/palette"

export const registerReactiveLogger = () =>
  createActor((sig) => {
    const jsonFormatter = createJsonFormatter({
      serializationFunction(line: SerializableLogEvent) {
        if (line.type === "clear") {
          sig.use(logsStore).clear()
        } else {
          sig.use(logsStore).addLogLine({
            node: "host",
            ...line,
          })
        }
        return ""
      },
    })

    captureConsole({ formatter: jsonFormatter, outputFunction: () => void 0 })

    if (process.env["DASSIE_LOG_TO_CLI"] === "true") {
      const cliFormatter = createCliFormatter()
      sig.use(logsStore).changes.on(sig, (action) => {
        switch (action[0]) {
          case "clear": {
            break
          }
          case "addLogLine": {
            const { node, type, date, message, parameters } = action[1][0]
            const prefix = node
              ? chalk.hex(selectBySeed(COLORS, node))(node) + " "
              : ""
            process.stdout.write(
              prefix +
                cliFormatter({
                  type,
                  message,
                  date: new Date(date),
                  parameters,
                }) +
                "\n"
            )
            break
          }
        }
      })
    }
  })
