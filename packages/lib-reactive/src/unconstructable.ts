export const createUnconstructable = <T>(): T => {
  throw new Error(
    "This value is not constructable and must be injected into the Reactor before use",
  )
}
