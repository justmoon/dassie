import type { StreamState } from "./state"

export class Stream {
  constructor(
    private readonly streamState: StreamState,
    public readonly id: number,
  ) {}

  send(amount: bigint) {
    this.streamState.sendMaximum += amount
  }
}
