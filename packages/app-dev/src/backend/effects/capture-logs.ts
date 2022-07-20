import type { EffectContext } from "@xen-ilp/lib-reactive"

import { indexedLogLineTopic } from "../features/logs"
import { logsStore } from "../stores/logs"

export const captureLogs = (sig: EffectContext) => {
  sig.on(indexedLogLineTopic, (line) => {
    sig.emit(logsStore, (draft) => {
      if (line.level === "clear") {
        draft.splice(0, draft.length)
      } else {
        draft.push(line)
      }
    })
  })
}
