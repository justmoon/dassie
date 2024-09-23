import type { Promisable } from "type-fest"

import { isThenable } from "@dassie/lib-type-utils"

import type { ScopeContext } from "../types/scope-context"
import type { StatefulContext } from "../types/stateful-context"

export interface WrappedCallback<
  TCallback extends (...parameters: unknown[]) => Promisable<unknown>,
> {
  (...parameters: Parameters<TCallback>): void
  original: TCallback
}

export const wrapCallback = <
  TCallback extends (...parameters: unknown[]) => Promisable<unknown>,
>(
  callback: TCallback,
  context: ScopeContext & StatefulContext<object>,
  path?: string,
): WrappedCallback<TCallback> => {
  const wrappedCallback: WrappedCallback<TCallback> = Object.assign(
    ((...parameters) => {
      if (context.scope.isDisposed) {
        context.reactor.debug?.warnAboutDanglingCallback(
          callback.name,
          context,
          path,
        )

        return
      }

      try {
        const result = callback(...parameters)

        if (isThenable(result)) {
          result.then(undefined, (error: unknown) => {
            console.error("error in async callback", {
              name: callback.name || "anonymous",
              path,
              error,
            })
          })
        }
      } catch (error) {
        console.error("error in callback", {
          name: callback.name || "anonymous",
          path,
          error,
        })
      }
    }) as TCallback,
    {
      original: callback,
    },
  )

  return wrappedCallback
}
