import { ReadonlyTopic, TopicSymbol } from "../topic"
import { LifecycleContext } from "../types/lifecycle-context"
import { FactoryNameSymbol } from "./context-base"
import {
  Listener,
  ListenerNameSymbol,
  emitToListener,
} from "./emit-to-listener"
import { Reactive, ReactiveSource, defaultComparator } from "./reactive"

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

export const createReactiveTopic = <T>(
  source: ReactiveSource<T>,
): ReadonlyTopic<T> => {
  let proxy: ReactiveProxy<T> | undefined = undefined
  let listeners: Listener<T> | Set<Listener<T>> | undefined

  const emit = (value: T) => {
    if (!listeners) return

    if (typeof listeners === "function") {
      emitToListener(topic[FactoryNameSymbol], listeners, value)
      return
    }

    for (const listener of listeners) {
      emitToListener(topic[FactoryNameSymbol], listener, value)
    }
  }

  const createProxy = () => {
    return new ReactiveProxy<T>(source, emit)
  }

  const topic: ReadonlyTopic<T> = {
    [FactoryNameSymbol]: "anonymous",
    [TopicSymbol]: true as const,

    on(context: LifecycleContext, listener: Listener<T>): void {
      if (import.meta.env.DEV) {
        Object.defineProperty(listener, ListenerNameSymbol, {
          value: context.lifecycle.name,
          enumerable: false,
        })
      }

      if (typeof listeners === "function") {
        listeners = new Set([listeners, listener])
      } else if (listeners == undefined) {
        proxy = createProxy()
        proxy.read()
        listeners = listener
      } else {
        listeners.add(listener)
      }

      context.lifecycle.onCleanup(() => {
        topic.off(listener)
      })
    },

    once(context: LifecycleContext, listener: Listener<T>): void {
      const singleUseListener = (message: T) => {
        topic.off(singleUseListener)
        return listener(message)
      }

      topic.on(context, singleUseListener)

      context.lifecycle.onCleanup(() => {
        topic.off(listener)
      })
    },

    off(listener: Listener<T>): void {
      if (typeof listeners === "function") {
        if (listeners === listener) {
          proxy?.removeParentObservers()
          proxy = undefined
          listeners = undefined
        }
      } else if (listeners != undefined) {
        listeners.delete(listener)

        if (listeners.size === 1) {
          // Generally it's better to use an iterator to get the first element from a set because it's more performant for large sets. But in this case the size of the set is always one so it's actually faster to use the array syntax.
          // See: https://tinyurl.com/perf-set-get-sole-element
          listeners = [...listeners][0]
        }
      }
    },
  }

  return topic
}
