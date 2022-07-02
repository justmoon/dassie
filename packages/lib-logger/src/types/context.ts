import type { Formatter } from "./formatter"

export interface LoggingContext {
  enableChecker: (component: string) => boolean
  formatter: Formatter
}
