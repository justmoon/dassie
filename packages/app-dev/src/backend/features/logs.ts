import type { SerializableLogLine } from "@xen-ilp/lib-logger"
import { EffectContext, createStore, createTopic } from "@xen-ilp/lib-reactive"

export interface NodeLogLine extends SerializableLogLine {
  node: string
}

export const logLineTopic = createTopic<NodeLogLine>("logLine")

export interface IndexedLogLine extends NodeLogLine {
  index: number
}

export const indexedLogLineTopic = createTopic<IndexedLogLine>("indexedLogLine")
export const currentLogIndexStore = createStore<number>("currentLogIndex", 0)

export const indexLogs = (sig: EffectContext) => {
  sig.on(logLineTopic, (line) => {
    const currentLogIndex = sig.read(currentLogIndexStore)
    sig.reactor.emit(currentLogIndexStore, (a) => a + 1)
    sig.reactor.emit(indexedLogLineTopic, {
      ...line,
      index: currentLogIndex,
    })
  })
}
