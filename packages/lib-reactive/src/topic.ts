import { isObject } from "@dassie/lib-type-utils"

import { ContextValue, FactoryNameSymbol } from "./internal/context-base"
import {
  Listener,
  ListenerNameSymbol,
  emitToListener,
} from "./internal/emit-to-listener"
import { LifecycleScope } from "./lifecycle"
import { Factory } from "./types/factory"

export const TopicSymbol = Symbol("das:reactive:topic")

export type InferMessageType<
  TTopic extends ReadonlyTopic<unknown> | Factory<ReadonlyTopic<unknown>>,
> = TTopic extends ReadonlyTopic<infer TMessage>
  ? TMessage
  : TTopic extends Factory<ReadonlyTopic<infer TMessage>>
  ? TMessage
  : never

export interface ReadonlyTopic<TMessage = never> extends ContextValue {
  /**
   * Marks this object as a topic.
   */
  [TopicSymbol]: true

  /**
   * Subscribe to a topic.
   *
   * @remarks
   *
   * Allows you to listen to events happening inside of the reactor. Every time a message is emitted on a topic, the listener will be called.
   *
   * Returns a function that when called will unsubscribe from the topic.
   *
   * @param listener - A function that will be called every time a message is emitted on the topic.
   * @returns A function which if called will unsubscribe the listener from the topic.
   */
  on: (
    this: void,
    lifecycle: LifecycleScope,
    listener: Listener<TMessage>,
  ) => void

  /**
   * Subscribe to receive a single message from a topic. Similar to {@link on} but will only call the listener once.
   *
   * @param listener - A function that will be called once the next time a message is emitted on the topic.
   * @returns A function which if called will unsubscribe the listener from the topic.
   */
  once: (
    this: void,
    lifecycle: LifecycleScope,
    listener: Listener<TMessage>,
  ) => void

  /**
   * Remove a listener from a topic.
   *
   * @param listener - The listener to remove.
   */
  off: (this: void, listener: Listener<TMessage>) => void
}

export type Topic<TMessage = never> = ReadonlyTopic<TMessage> & {
  /**
   * Emit a message to all listeners of a topic.
   *
   * @param trigger - Value to pass to the message factory function.
   */
  emit: (this: void, trigger: TMessage) => void
}

class TopicImplementation<TMessage> {
  [TopicSymbol] = true as const;
  [FactoryNameSymbol] = "anonymous"

  /**
   * The listeners for this topic.
   *
   * @remarks
   *
   * For performance reasons, this can be either undefined (no listeners), a function (one listener), or a set (multiple listeners).
   *
   * This avoids allocating a new set for every topic even though the topic may only have zero or one listeners.
   */
  private listeners: Listener<TMessage> | Set<Listener<TMessage>> | undefined

  emit = (message: TMessage) => {
    if (this.listeners == undefined) {
      return
    }

    if (typeof this.listeners === "function") {
      emitToListener(this[FactoryNameSymbol], this.listeners, message)
      return
    }

    for (const listener of this.listeners) {
      emitToListener(this[FactoryNameSymbol], listener, message)
    }
  }

  on = (lifecycle: LifecycleScope, listener: Listener<TMessage>) => {
    if (import.meta.env.DEV) {
      Object.defineProperty(listener, ListenerNameSymbol, {
        value: lifecycle.name,
        enumerable: false,
      })
    }

    if (typeof this.listeners === "function") {
      this.listeners = new Set([this.listeners, listener])
    } else if (this.listeners == undefined) {
      this.listeners = listener
    } else {
      this.listeners.add(listener)
    }

    lifecycle.onCleanup(() => {
      this.off(listener)
    })
  }

  once = (lifecycle: LifecycleScope, listener: Listener<TMessage>) => {
    const singleUseListener = (message: TMessage) => {
      this.off(singleUseListener)
      return listener(message)
    }

    this.on(lifecycle, singleUseListener)

    lifecycle.onCleanup(() => {
      this.off(listener)
    })
  }

  off = (listener: Listener<TMessage>) => {
    if (typeof this.listeners === "function") {
      if (this.listeners === listener) {
        this.listeners = undefined
      }
    } else if (this.listeners != undefined) {
      this.listeners.delete(listener)

      if (this.listeners.size === 1) {
        // Generally it's better to use an iterator to get the first element from a set because it's more performant for large sets. But in this case the size of the set is always one so it's actually faster to use the array syntax.
        // See: https://tinyurl.com/perf-set-get-sole-element
        this.listeners = [...this.listeners][0]
      }
    }
  }
}

export const createTopic = <TMessage = void>(): Topic<TMessage> => {
  return new TopicImplementation()
}

export const isTopic = (object: unknown): object is Topic =>
  isObject(object) && object[TopicSymbol] === true
