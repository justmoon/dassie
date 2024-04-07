export interface Deferred<T> extends Promise<T> {
  resolve(this: void, value: T): void
}

export const createDeferred = <T = void>(): Deferred<T> => {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((_resolve) => {
    resolve = _resolve
  })
  return Object.assign(promise, { resolve })
}
