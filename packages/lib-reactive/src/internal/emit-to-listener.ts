import { Promisable } from "type-fest"

export type Listener<TMessage> = (message: TMessage) => Promisable<void>

export const ListenerNameSymbol = Symbol("das:reactive:listener-name")

export const emitToListener = <TMessage>(
  emitterName: string,
  listener: Listener<TMessage>,
  message: TMessage,
) => {
  try {
    const result = listener(message)

    if (typeof result?.then === "function") {
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
