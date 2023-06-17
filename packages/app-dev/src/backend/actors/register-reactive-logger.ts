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

    const cliFormatter = createCliFormatter()
    if (process.env["DASSIE_LOG_TO_CLI"] === "true") {
      sig.use(logsStore).changes.on(sig, (action) => {
        if (action[0] === "addLogLine") {
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
        }
      })
    } else {
      // We still want to log warnings and errors from the host so the user
      // knows what happened in case something goes wrong with the web UI.
      sig.use(logsStore).changes.on(sig, (action) => {
        if (action[0] === "addLogLine") {
          const { node, type, date, message, parameters } = action[1][0]
          if (node !== "host") return
          if (type !== "warn" && type !== "error") return

          process.stdout.write(
            cliFormatter({
              type,
              message,
              date: new Date(date),
              parameters,
            }) + "\n"
          )
        }
      })
    }
  })
