import type { Listener, Topic } from "@dassie/lib-reactive"

import type { EventEmitter } from "../types/event-emitter"
import { generateCredentials } from "./generate-credentials"
import type { ServerEvents, ServerState } from "./state"

export class Server implements EventEmitter<ServerEvents> {
  constructor(private readonly state: ServerState) {}

  generateCredentials() {
    return generateCredentials(this.state)
  }

  on<TEventType extends keyof ServerEvents>(
    eventType: TEventType,
    handler: Listener<ServerEvents[TEventType]>,
  ) {
    const topic: Topic<ServerEvents[TEventType]> = this.state.topics[eventType]
    topic.on(undefined, handler)
  }

  off(eventType: keyof ServerEvents, handler: Listener<unknown>) {
    this.state.topics[eventType].off(handler)
  }
}
