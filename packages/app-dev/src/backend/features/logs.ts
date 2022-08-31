import type { SerializableLogLine } from "@dassie/lib-logger"
import { EffectContext, createStore, createTopic } from "@dassie/lib-reactive"

export interface NodeLogLine extends SerializableLogLine {
  node: string
}

export const logLineTopic = () => createTopic<NodeLogLine>()

export interface IndexedLogLine extends NodeLogLine {
  index: number
  relativeTime: number
}

export const indexedLogLineTopic = () => createTopic<IndexedLogLine>()
export const currentLogIndexStore = () => createStore(0)

const startupTime = Date.now()

export const indexLogs = (sig: EffectContext) => {
  sig.on(logLineTopic, (line) => {
    const currentLogIndex = sig.use(currentLogIndexStore).read()
    sig.use(currentLogIndexStore).update((a) => a + 1)
    sig.use(indexedLogLineTopic).emit({
      ...line,
      index: currentLogIndex,
      relativeTime: Number(new Date(line.date)) - startupTime,
    })
  })
}
