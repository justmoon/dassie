export interface NodejsSocket {
  on(eventType: "data", handler: (data: string) => void): void
  on(eventType: "close", handler: () => void): void
  write(data: string): void
  end(): void
  setEncoding(encoding: "utf8"): void
}
