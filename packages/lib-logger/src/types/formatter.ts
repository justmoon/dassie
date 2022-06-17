export interface Formatter {
  clear(): void
  log(level: string, message: string, data?: Record<string, unknown>): void
}

export interface FormatterConstructor {
  new (component: string): Formatter
}
