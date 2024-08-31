import type { StreamState } from "./state"

export class Stream {
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

  on(eventType: "money", handler: (amount: bigint) => void) {
    this.state.topics[eventType].on(undefined, handler)
  }
}
