import produce from "immer"

import type { EffectContext } from "@xen-ilp/lib-reactive"

import { logsStore } from "../stores/logs"
import { logLineTopic } from "../topics/log-message"

export const captureLogs = (sig: EffectContext) => {
  sig.on(logLineTopic, (line) => {
    sig.reactor.emit(
      logsStore,
      produce((draft) => {
        if (line.level === "clear") {
          draft.splice(0, draft.length)
        } else {
          draft.push(line)
        }
      })
    )
  })
}
