import { isObject } from "@dassie/lib-type-utils"

import type { ActorContext } from "./context"
import { Factory, FactoryNameSymbol } from "./reactor"
import { Signal, createSignal } from "./signal"

export type Behavior<TReturn = unknown, TProperties = unknown> = (
  sig: ActorContext,
  properties: TProperties
) => TReturn

export const ActorSymbol = Symbol("das:reactive:actor")

export type ActorFactory<TInstance, TProperties = undefined> = Factory<
  Actor<TInstance, TProperties>
>

export type InferActorMessageType<TInstance> = TInstance extends (
  message: infer TMessage
) => unknown
  ? TMessage
  : never

export type InferActorReturnType<TInstance> = TInstance extends (
  ...parameters: never[]
) => infer TReturn
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
   * The function that initializes the actor. Used internally. You should call reactor.run() or sig.run() if you want to start the actor.
   */
  behavior: Behavior<TInstance, TProperties>

  /**
   * Fire-and-forget message to the actor. The message will be delivered asynchronously and any return value will be ignored.
   *
   * @param message - The message to send to the actor.
   */
  tell: (message: InferActorMessageType<TInstance>) => void

  /**
   * Asynchronously message the actor and receive a response as a promise.
   *
   * @param message - The message to send to the actor.
   * @returns A promise that will resolve with the response from the actor.
   */
  ask: (
    message: InferActorMessageType<TInstance>
  ) => Promise<InferActorReturnType<TInstance>>
}

export const createActor = <TInstance, TProperties = undefined>(
  behavior: Behavior<TInstance, TProperties>
): Actor<TInstance, TProperties> => {
  const signal = createSignal<Awaited<TInstance>>()
  const actor: Actor<TInstance, TProperties> = {
    ...signal,
    [ActorSymbol]: true,
    behavior,
    tell: (message) => {
      const handler = signal.read()

      if (handler) {
        try {
          const result = (handler as (parameter: typeof message) => unknown)(
            message
          )

          if (
            isObject(result) &&
            "catch" in result &&
            typeof result["catch"] === "function"
          ) {
            ;(result as unknown as Promise<unknown>).catch((error: unknown) => {
              console.error("error in actor", {
                error,
                actor: actor[FactoryNameSymbol],
              })
            })
          }
        } catch (error: unknown) {
          console.error("error in actor", {
            error,
            actor: actor[FactoryNameSymbol],
          })
        }

        return
      }

      const removeListener = signal.on((handler) => {
        if (handler) {
          try {
            const result = (handler as (parameter: typeof message) => unknown)(
              message
            )

            if (
              isObject(result) &&
              "catch" in result &&
              typeof result["catch"] === "function"
            ) {
              ;(result as unknown as Promise<unknown>).catch(
                (error: unknown) => {
                  console.error("error in actor", {
                    error,
                    actor: actor[FactoryNameSymbol],
                  })
                }
              )
            }

            return
          } catch (error: unknown) {
            console.error("error in actor", {
              error,
              actor: actor[FactoryNameSymbol],
            })
          } finally {
            removeListener()
          }
        }
      })
    },
    ask: (message) => {
      const handler = signal.read()

      return new Promise((resolve, reject) => {
        if (handler) {
          resolve(
            (
              handler as (
                parameter: InferActorMessageType<TInstance>
              ) => InferActorReturnType<TInstance>
            )(message)
          )
          return
        }

        const removeListener = signal.on((handler) => {
          if (handler) {
            try {
              resolve(
                (
                  handler as (
                    parameter: InferActorMessageType<TInstance>
                  ) => InferActorReturnType<TInstance>
                )(message)
              )
            } catch (error) {
              reject(error)
            } finally {
              removeListener()
            }
          }
        })
      })
    },
  }

  return actor
}

export const isActor = (object: unknown): object is Actor<unknown, unknown> =>
  isObject(object) && object[ActorSymbol] === true
