import {
  CliFormatter,
  Formatter,
  JsonFormatter,
  SerializableLogLine,
  createEnableChecker,
  createLogger,
  createLoggerFactory,
} from "@xen-ilp/lib-logger"

import { logLineTopic } from "../topics/log-message"
import eventBroker from "./event-broker"

export class DevelopmentServerLogFormatter implements Formatter {
  private cliFormatter = new CliFormatter()
  private jsonFormatter = new JsonFormatter({
    outputFunction(line: SerializableLogLine) {
      eventBroker.emit(logLineTopic, {
        node: "",
        ...line,
      })
    },
  })

  clear() {
    this.cliFormatter.clear()
    this.jsonFormatter.clear()
  }

  log(...parameters: Parameters<Formatter["log"]>) {
    this.cliFormatter.log(...parameters)
    this.jsonFormatter.log(...parameters)
  }
}

export const register = () => {
  createLogger.setFormatter(new DevelopmentServerLogFormatter())
}

export const createCliOnlyLogger = createLoggerFactory({
  enableChecker: createEnableChecker(process.env["DEBUG"] ?? ""),
  formatter: new CliFormatter(),
})
