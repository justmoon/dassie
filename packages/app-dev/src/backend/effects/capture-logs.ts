import type { EffectContext } from "@dassie/lib-reactive"

import { logsStore } from "../../common/stores/logs"
import { indexedLogLineTopic } from "../features/logs"

export const captureLogs = (sig: EffectContext) => {
  sig.on(indexedLogLineTopic, (line) => {
    const logs = sig.use(logsStore)
    if (line.level === "clear") {
      logs.clear()
    } else {
      logs.addLogLine(line)
    }
  })
}
