import type { ConditionalKeys } from "type-fest"

import { isObject } from "@dassie/lib-type-utils"

import type { ActorContext } from "./context"
import { SettablePromise, makePromise } from "./internal/promise"
import { FactoryNameSymbol } from "./reactor"
import { type Signal, createSignal } from "./signal"

export type Behavior<TReturn = unknown, TProperties = unknown> = (
  sig: ActorContext,
  properties: TProperties
) => TReturn

export const ActorSymbol = Symbol("das:reactive:actor")

export type MessageHandler = (message: never) => unknown

export type MessageHandlerRecord = Record<string, MessageHandler>

export type InferActorMessageType<
  TInstance,
  TMethod extends string & keyof TInstance
> = TInstance[TMethod] extends (message: infer TMessage) => unknown
  ? TMessage
  : never

export type InferActorReturnType<
  TInstance,
  TMethod extends string & keyof TInstance
> = TInstance[TMethod] extends (...parameters: never[]) => infer TReturn
  ? TReturn
  : never

export type Actor<TInstance, TProperties = undefined> = Signal<
  Awaited<TInstance> | undefined
> & {
  /**
   * Marks this object as a value.
   */
  [ActorSymbol]: true

  /**
   * Whether the actor is currently instantiated inside of a reactor.
   */
  isRunning: boolean

  /**
   * The result of the most recent succesful execution of the actor.
   *
   * Note that this is the raw result which may be a promise. If you want to get the resolved value, use `actor.read()`.
   *
   * If the actor hasn't run yet, the result will be undefined.
   * If the actor throws an error, the result will be a rejected promise which is represented by a Promise<never> type.
   */
  result: TInstance | undefined | Promise<never>

  promise: SettablePromise<TInstance>

  /**
   * The function that initializes the actor. Used internally. You should call reactor.run() or sig.run() if you want to start the actor.
   */
  behavior: Behavior<TInstance, TProperties>

  /**
   * Fire-and-forget message to the actor. The message will be delivered asynchronously and any return value will be ignored.
   *
   * @param message - The message to send to the actor.
   */
  tell: <
    TMethod extends string & ConditionalKeys<Awaited<TInstance>, MessageHandler>
  >(
    this: void,
    method: TMethod,
    message: InferActorMessageType<Awaited<TInstance>, TMethod>
  ) => void

  /**
   * Asynchronously message the actor and receive a response as a promise.
   *
   * @param message - The message to send to the actor.
   * @returns A promise that will resolve with the response from the actor.
   */
  ask: <
    TMethod extends string & ConditionalKeys<Awaited<TInstance>, MessageHandler>
  >(
    this: void,
    method: TMethod,
    message: InferActorMessageType<Awaited<TInstance>, TMethod>
  ) => Promise<InferActorReturnType<Awaited<TInstance>, TMethod>>
}

type CreateActorSignature = <TInstance, TProperties = undefined>(
  behavior: Behavior<TInstance, TProperties>
) => Actor<TInstance, TProperties>

export const createActor: CreateActorSignature = <
  TInstance,
  TProperties = undefined
>(
  behavior: Behavior<TInstance, TProperties>
): Actor<TInstance, TProperties> => {
  const signal = createSignal<Awaited<TInstance> | undefined>(undefined)
  const actor: Actor<TInstance, TProperties> = {
    ...signal,
    [ActorSymbol]: true,
    isRunning: false,
    result: undefined,
    promise: makePromise(),
    behavior,
    tell: (method, message) => {
      actor.ask(method, message).catch((error: unknown) => {
        console.error("error in actor handler", {
          error,
          actor: actor[FactoryNameSymbol],
          method,
          message,
        })
      })
    },
    ask: async (method, message) => {
      if (import.meta.env.DEV && !actor.isRunning) {
        debugOptimisticActorCall(
          actor.promise,
          actor[FactoryNameSymbol],
          method
        )
      }

      const actorInstance = await actor.promise

      return (
        actorInstance[method] as (
          parameter: typeof message
        ) => InferActorReturnType<Awaited<TInstance>, typeof method>
      )(message)
    },
  }

  return actor
}

export const isActor = (
  object: unknown
): object is Actor<MessageHandlerRecord, unknown> =>
  isObject(object) && object[ActorSymbol] === true

const DEBUG_ACTOR_OPTIMISTIC_CALL_TIMEOUT = 2000
export const debugOptimisticActorCall = (
  actorPromise: Promise<unknown>,
  actorName: string,
  method: string
) => {
  const timeout = setTimeout(() => {
    console.error("Actor called but never instantiated", {
      actor: actorName,
      method,
    })
  }, DEBUG_ACTOR_OPTIMISTIC_CALL_TIMEOUT)

  actorPromise.finally(() => clearTimeout(timeout))
}
