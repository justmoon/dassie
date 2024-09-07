import type { Topic } from "@dassie/lib-reactive"

export type InferTopics<TEventTypes extends Record<string, unknown>> = {
  [K in keyof TEventTypes]: Topic<TEventTypes[K]>
}
