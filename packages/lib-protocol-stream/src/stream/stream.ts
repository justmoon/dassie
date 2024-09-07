import { type Listener, type Topic } from "@dassie/lib-reactive"

import type { EventEmitter } from "../types/event-emitter"
import type { StreamEvents, StreamState } from "./state"

export class Stream implements EventEmitter<StreamEvents> {
  constructor(
    private readonly state: StreamState,
    public readonly id: number,
  ) {}

  send(amount: bigint) {
    this.state.sendMaximum += amount
  }

  receive(amount: bigint) {
    this.state.receiveMaximum += amount
  }

  on<TEventType extends keyof StreamEvents>(
    eventType: TEventType,
    handler: Listener<StreamEvents[TEventType]>,
  ) {
    const topic: Topic<StreamEvents[TEventType]> = this.state.topics[eventType]
    topic.on(undefined, handler)
  }

  off(eventType: keyof StreamEvents, handler: Listener<unknown>) {
    this.state.topics[eventType].off(handler)
  }
}
