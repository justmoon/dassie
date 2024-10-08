import { isObject } from "@dassie/lib-type-utils"

import { createDeferred } from "./deferred"
import { type ContextValue, FactoryNameSymbol } from "./internal/context-base"
import {
  type Listener,
  ListenerNameSymbol,
  emitToListener,
} from "./internal/emit-to-listener"
import { type Scope, createScope } from "./scope"
import type { Factory } from "./types/factory"
import type { ScopeContext } from "./types/scope-context"

export const TopicSymbol = Symbol("das:reactive:topic")

export type InferTopicType<
  TTopic extends ReadonlyTopic<unknown> | Factory<ReadonlyTopic<unknown>>,
> =
  TTopic extends ReadonlyTopic<infer TMessage> ? TMessage
  : TTopic extends Factory<ReadonlyTopic<infer TMessage>> ? TMessage
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
    scope: ScopeContext | Scope | undefined,
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
    scope: ScopeContext | Scope | undefined,
    listener: Listener<TMessage>,
  ) => void

  /**
   * Remove a listener from a topic.
   *
   * @param listener - The listener to remove.
   */
  off: (this: void, listener: Listener<TMessage>) => void

  /**
   * Return an async iterable for this topic.
   *
   * @returns An async iterable that will yield messages as they are emitted.
   */
  [Symbol.asyncIterator]: () => AsyncIterator<TMessage>

  /**
   * Return a promise which will resolve to the next value in the topic.
   *
   * @returns A promise which will resolve to the next value in the topic.
   */
  next: () => Promise<TMessage>
}

export type Topic<TMessage = never> = ReadonlyTopic<TMessage> & {
  /**
   * Emit a message to all listeners of a topic.
   *
   * @param trigger - Value to pass to the message factory function.
   */
  emit: (this: void, trigger: TMessage) => void
}

export class TopicImplementation<TMessage> {
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

  on = (
    context: ScopeContext | Scope | undefined,
    listener: Listener<TMessage>,
  ) => {
    const scope = context && "scope" in context ? context.scope : context

    if (import.meta.env.DEV && scope) {
      Object.defineProperty(listener, ListenerNameSymbol, {
        value: scope.name,
        enumerable: false,
      })
    }

    if (typeof this.listeners === "function") {
      this.listeners = new Set([this.listeners, listener])
    } else if (this.listeners == undefined) {
      this.enable?.()
      this.listeners = listener
    } else {
      this.listeners.add(listener)
    }

    scope?.onCleanup(() => {
      this.off(listener)
    })
  }

  once = (
    context: ScopeContext | Scope | undefined,
    listener: Listener<TMessage>,
  ) => {
    const singleUseListener = (message: TMessage) => {
      this.off(singleUseListener)
      return listener(message)
    }

    this.on(context, singleUseListener)

    const scope = context && "scope" in context ? context.scope : context
    scope?.onCleanup(() => {
      this.off(listener)
    })
  }

  off = (listener: Listener<TMessage>) => {
    if (typeof this.listeners === "function") {
      if (this.listeners === listener) {
        this.listeners = undefined
        this.disable?.()
      }
    } else if (this.listeners != undefined) {
      this.listeners.delete(listener)

      if (this.listeners.size === 1) {
        // Generally it's better to use an iterator to get the first element from a set because it's more performant for large sets. But in this case the size of the set is always one so it's actually faster to use the array syntax.
        // See: https://tinyurl.com/perf-set-get-sole-element
        this.listeners = [...this.listeners][0]
      }
    }
  };

  [Symbol.asyncIterator] = (): AsyncIterator<TMessage> => {
    const queue: TMessage[] = []
    let promise = createDeferred()

    const scope = createScope("topic-async-iterator")

    this.on({ scope }, (message) => {
      queue.push(message)
      promise.resolve()
    })

    return {
      next: async () => {
        if (scope.isDisposed) {
          return { done: true, value: undefined }
        }

        while (queue.length === 0) {
          await promise
          promise = createDeferred()
        }

        const value = queue.shift()!
        return { done: false, value }
      },
      return: async () => {
        await scope.dispose()
        return { done: true, value: undefined }
      },
    }
  }

  next = () => {
    return new Promise<TMessage>((resolve) => {
      this.once({ scope: createScope("topic-next") }, resolve)
    })
  }

  /**
   * Method that will be called when a listener is added when there were none.
   *
   * Can be used to implement subclasses which only do work when there are listeners.
   */
  protected enable?(): void

  /**
   * Method that will be called when the last listener is removed.
   *
   * Can be used to implement subclasses which only do work when there are listeners.
   */
  protected disable?(): void
}

export const createTopic = <TMessage = void>(): Topic<TMessage> => {
  return new TopicImplementation()
}

export const isTopic = (object: unknown): object is Topic =>
  isObject(object) && object[TopicSymbol] === true
