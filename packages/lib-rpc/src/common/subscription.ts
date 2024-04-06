export type Disposer = () => void
export type Subscription<T> = (onData: (value: T) => void) => Disposer

export function createSubscription<T>(
  subscribe: Subscription<T>,
): Subscription<T> {
  return subscribe
}
