import { isObject } from "@dassie/lib-type-utils"

import { Disposer, Factory, FactoryNameSymbol } from "./reactor"

export type Listener<TMessage> = (message: TMessage) => void

export const TopicSymbol = Symbol("das:reactive:topic")

export type InferMessageType<TTopic extends Factory<ReadonlyTopic<unknown>>> =
  TTopic extends Factory<ReadonlyTopic<infer TMessage>> ? TMessage : never

export interface ReadonlyTopic<TMessage = never> {
  /**
   * Marks this object as a topic.
   */
  [TopicSymbol]: true

  /**
   * Name of the factory function that created this topic.
   *
   * @see {@link FactoryNameSymbol}
   */
  [FactoryNameSymbol]: string

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
  on: (this: void, listener: Listener<TMessage>) => Disposer

  /**
   * Subscribe to receive a single message from a topic. Similar to {@link on} but will only call the listener once.
   *
   * @param listener - A function that will be called once the next time a message is emitted on the topic.
   * @returns A function which if called will unsubscribe the listener from the topic.
   */
  once: (this: void, listener: Listener<TMessage>) => Disposer
}

export type Topic<TMessage = never> = ReadonlyTopic<TMessage> & {
  /**
   * Emit a message to all listeners of a topic.
   *
   * @param trigger - Value to pass to the message factory function.
   */
  emit: (this: void, trigger: TMessage) => void
}

export const createTopic = <TMessage>(): Topic<TMessage> => {
  // We construct a temporary object in order to assign the name to the function
  let listeners: Listener<TMessage> | Set<Listener<TMessage>> | undefined

  const emit = (message: TMessage) => {
    if (typeof listeners === "function") {
      try {
        listeners(message)
      } catch (error) {
        console.error("error in listener", {
          topic: topic[FactoryNameSymbol],
          error,
        })
      }
      return
    } else if (listeners == undefined) {
      return
    }

    for (const listener of listeners) {
      try {
        listener(message)
      } catch (error) {
        console.error("error in listener", {
          topic: topic[FactoryNameSymbol],
          error,
        })
      }
    }
  }

  const on = (listener: Listener<TMessage>) => {
    if (typeof listeners === "function") {
      listeners = new Set([listeners, listener])
    } else if (listeners == undefined) {
      listeners = listener
    } else {
      listeners.add(listener)
    }

    return () => {
      if (typeof listeners === "function") {
        if (listeners === listener) {
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
    }
  }

  const once = (listener: Listener<TMessage>) => {
    const disposer = on((message) => {
      disposer()
      listener(message)
    })

    return disposer
  }

  const topic: Topic<TMessage> = {
    [TopicSymbol]: true,
    [FactoryNameSymbol]: "anonymous",
    on,
    once,
    emit,
  }

  return topic
}

export const isTopic = (object: unknown): object is Topic =>
  isObject(object) && object[TopicSymbol] === true
