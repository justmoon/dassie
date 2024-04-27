export interface NodejsMessagePort {
  close(): void
  postMessage(message: unknown): void
  on(event: "message", handler: (message: unknown) => void): void
  on(event: "error", handler: (error: unknown) => void): void
  on(event: "close", handler: () => void): void
  off(event: "message", handler: (message: unknown) => void): void
  off(event: "error", handler: (error: unknown) => void): void
  off(event: "close", handler: () => void): void
}
