import { defaultSelector } from "./internal/default-selector"
import {
  ReactiveSelector,
  type ReactiveSource,
  defaultComparator,
} from "./internal/reactive"
import type { Reactor } from "./reactor"
import type { FactoryOrInstance } from "./types/factory"
import type { ReactiveContext } from "./types/reactive-context"
import type { StatefulContext } from "./types/stateful-context"

export class ReactiveContextImplementation<TBase extends object>
  implements StatefulContext<TBase>, ReactiveContext<TBase>
{
  constructor(
    /**
     * A reference to the current reactor.
     *
     * @remarks
     *
     * If you want to pass something to an external component to allow that component to interact with the reactive system, you can use this reference.
     */
    readonly reactor: Reactor<TBase>,

    /**
     * A function which allows tracked reads of signals.
     */
    readonly _get: <TState>(signal: ReactiveSource<TState>) => TState,
  ) {}

  readAndTrack<TState, TSelection>(
    signalFactory: FactoryOrInstance<ReactiveSource<TState>, TBase>,
    // Based on the overloaded function signature, the selector parameter may be omitted iff TMessage equals TSelection.
    // Therefore this cast is safe.
    selector: (state: TState) => TSelection = defaultSelector as unknown as (
      state: TState,
    ) => TSelection,
    comparator: (a: TSelection, b: TSelection) => boolean = defaultComparator,
  ) {
    const signal =
      typeof signalFactory === "function" ?
        this.reactor.use(signalFactory)
      : signalFactory

    if (selector === defaultSelector && comparator === defaultComparator) {
      return this._get(signal)
    } else {
      const intermediateSignal = new ReactiveSelector(
        signal,
        selector,
        comparator,
      )
      return this._get(intermediateSignal)
    }
  }

  readKeysAndTrack<TState extends object, TKeys extends keyof TState>(
    signal: FactoryOrInstance<ReactiveSource<TState>, TBase>,
    keys: readonly TKeys[],
  ): Pick<TState, TKeys> {
    return this.readAndTrack(
      signal,
      (state) => {
        const result = {} as Pick<TState, TKeys>
        for (const key of keys) {
          result[key] = state[key]
        }
        return result
      },
      (a, b) => {
        for (const key of keys) {
          if (a[key] !== b[key]) {
            return false
          }
        }
        return true
      },
    )
  }

  read<TState>(
    signalFactory: FactoryOrInstance<ReactiveSource<TState>, TBase>,
  ): TState {
    const signal =
      typeof signalFactory === "function" ?
        this.reactor.use(signalFactory)
      : signalFactory

    return signal.read()
  }
}
