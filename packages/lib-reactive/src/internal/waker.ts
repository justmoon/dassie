export const createWaker = (): readonly [
  promise: Promise<void>,
  resolve: () => void
] => {
  let resolve!: () => void
  const promise = new Promise<void>((_resolve) => {
    resolve = _resolve
  })
  return [promise, resolve] as const
}
