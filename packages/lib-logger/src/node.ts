import { getLogContext } from "./context"
import { createEnableChecker } from "./enabled"
import { createCliFormatter } from "./formatters/cli-formatter"

export * from "./common"

const cliFormatter = createCliFormatter()

export const context = getLogContext()

context.enableChecker = createEnableChecker(process.env["DEBUG"] ?? "")
context.output = (logEvent) => {
  process.stdout.write(cliFormatter(logEvent))
  process.stdout.write("\n")
}
