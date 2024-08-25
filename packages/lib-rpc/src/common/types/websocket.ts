export interface WebSocketImplementation {
  addEventListener(
    eventType: "message",
    handler: (event: MessageEvent) => void,
  ): void
  addEventListener(eventType: "open" | "close", handler: () => void): void
  send(data: string): void
  close(): void
  readyState: number
}

export interface MessageEvent {
  data: string | Uint8Array | ArrayBuffer | Buffer[]
}

export type WebSocketConstructor = new (url: string) => WebSocketImplementation
