import {
  createCliFormatter,
  createEnableChecker,
  createLoggerFactory,
} from "@dassie/lib-logger"

export const createCliOnlyLogger = createLoggerFactory({
  enableChecker: createEnableChecker(process.env["DEBUG"] ?? ""),
  formatter: createCliFormatter(),
})
