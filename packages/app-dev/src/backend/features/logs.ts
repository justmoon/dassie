import type { SerializableLogLine } from "@xen-ilp/lib-logger"
import { EffectContext, createStore, createTopic } from "@xen-ilp/lib-reactive"

export interface NodeLogLine extends SerializableLogLine {
  node: string
}

export const logLineTopic = () => createTopic<NodeLogLine>()

export interface IndexedLogLine extends NodeLogLine {
  index: number
}

export const indexedLogLineTopic = () => createTopic<IndexedLogLine>()
export const currentLogIndexStore = () => createStore<number>(0)

export const indexLogs = (sig: EffectContext) => {
  sig.on(logLineTopic, (line) => {
    const currentLogIndex = sig.read(currentLogIndexStore)
    sig.emit(currentLogIndexStore, (a) => a + 1)
    sig.emit(indexedLogLineTopic, {
      ...line,
      index: currentLogIndex,
    })
  })
}
