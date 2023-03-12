import type { EffectContext } from "@dassie/lib-reactive"

import { registerStatisticsHttpHandler } from "./register-statistics-http-handler"

export const startStatisticsServer = (sig: EffectContext) => {
  sig.run(registerStatisticsHttpHandler)
}
