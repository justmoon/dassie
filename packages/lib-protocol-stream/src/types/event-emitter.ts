import type { Listener } from "@dassie/lib-reactive"

export type EventEmitter<TEventTypes extends Record<string, unknown>> = {
  on<TEventType extends keyof TEventTypes>(
    eventType: TEventType,
    listener: (event: TEventTypes[TEventType]) => void,
  ): void

  off(eventType: keyof TEventTypes, listener: Listener<unknown>): void
}
