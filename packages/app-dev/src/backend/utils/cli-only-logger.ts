import {
  createCliFormatter,
  createEnableChecker,
  createLoggerFactory,
} from "@xen-ilp/lib-logger"

export const createCliOnlyLogger = createLoggerFactory({
  enableChecker: createEnableChecker(process.env["DEBUG"] ?? ""),
  formatter: createCliFormatter(),
})
