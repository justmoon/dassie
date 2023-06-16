export const Canceled = Symbol("Canceled")
export type Canceled = typeof Canceled

export const isCanceled = (value: unknown): value is Canceled =>
  value === Canceled
