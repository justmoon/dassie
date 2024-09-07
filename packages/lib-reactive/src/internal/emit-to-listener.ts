import type { Promisable } from "type-fest"

import { isThenable } from "@dassie/lib-type-utils"

export type Listener<TMessage> = (message: TMessage) => Promisable<void>

export const ListenerNameSymbol = Symbol("das:reactive:listener-name")

export const emitToListener = <TMessage>(
  emitterName: string,
  listener: Listener<TMessage>,
  message: TMessage,
) => {
  try {
    const result = listener(message)

    if (isThenable(result)) {
      result.then(undefined, (error: unknown) => {
        console.error("error in async listener", {
          emitter: emitterName,
          scope:
            (listener as { [ListenerNameSymbol]?: string })[
              ListenerNameSymbol
            ] ?? "anonymous",
          error,
        })
      })
    }
  } catch (error) {
    console.error("error in listener", {
      topic: emitterName,
      scope:
        (listener as { [ListenerNameSymbol]?: string })[ListenerNameSymbol] ??
        "anonymous",
      error,
    })
  }
}
