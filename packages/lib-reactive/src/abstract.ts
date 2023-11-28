export const createAbstract = <T>(): T => {
  throw new Error(
    "This value is abstract and therefore must be explicitly injected into the Reactor before use",
  )
}
