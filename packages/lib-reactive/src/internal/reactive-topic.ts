import { type ReadonlyTopic, TopicImplementation } from "../topic"
import { Reactive, type ReactiveSource, defaultComparator } from "./reactive"

class ReactiveProxy<T> extends Reactive<T> {
  constructor(
    private readonly source: ReactiveSource<T>,
    private readonly emit: (value: T) => void,
  ) {
    super(defaultComparator, true)
  }

  recompute() {
    this.cache = this.readWithTracking(this.source)
    this.emit(this.readWithTracking(this.source))
  }
}

class ReactiveTopicImplementation<
  TMessage,
> extends TopicImplementation<TMessage> {
  #proxy: ReactiveProxy<TMessage> | undefined

  constructor(private readonly source: ReactiveSource<TMessage>) {
    super()
  }

  override enable() {
    this.#proxy = new ReactiveProxy<TMessage>(this.source, this.emit)
    this.#proxy.read()
  }

  override disable() {
    this.#proxy?.removeParentObservers()
    this.#proxy = undefined
  }
}

export const createReactiveTopic = <T>(
  source: ReactiveSource<T>,
): ReadonlyTopic<T> => {
  return new ReactiveTopicImplementation(source)
}
