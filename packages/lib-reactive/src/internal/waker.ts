export class Waker {
  readonly promise: Promise<void>
  readonly resolve: () => void
  private isDisposed = false

  constructor() {
    let _resolve!: () => void
    this.promise = new Promise<void>((resolve) => {
      _resolve = resolve
    })
    this.resolve = _resolve
  }

  get notify() {
    if (this.isDisposed) {
      throw new Error(
        "You tried to access a tracked property on an effect that has already finished running.\n\nPlease note that you can only use tracked methods like get() in the effect that they were passed in to. You should not access them from closures or pass them as a parameter."
      )
    }

    return this.resolve
  }

  dispose() {
    this.isDisposed = true
  }
}
