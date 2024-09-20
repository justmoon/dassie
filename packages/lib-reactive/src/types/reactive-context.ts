import type { ReactiveSource } from "../internal/reactive"
import type { FactoryOrInstance } from "./factory"

export interface ReactiveContext<TBase extends object> {
  /**
   * Read the current value from a signal but recompute if the value changes.
   *
   * @remarks
   *
   * To read a value without tracking, just get a reference and call `read()` on it.
   *
   * @param signalFactory - Reference to the signal's factory function.
   * @param selector - Used to select only part of the value from a given signal. This can be useful to avoid re-running if only an irrelevant portion of the value has changed.
   * @param comparator - By default, the reactor checks for strict equality (`===`) to determine whether the value has changed. This can be overridden by passing a custom comparator function.
   * @returns The current value of the signal, narrowed by the selector.
   */
  readAndTrack<TState>(
    signalFactory: FactoryOrInstance<ReactiveSource<TState>, TBase>,
  ): TState
  readAndTrack<TState, TSelection>(
    signalFactory: FactoryOrInstance<ReactiveSource<TState>, TBase>,
    selector: (state: TState) => TSelection,
    comparator?: (oldValue: TSelection, newValue: TSelection) => boolean,
  ): TSelection

  /**
   * Convenience method for extracting specific keys from a signal.
   *
   * @remarks
   *
   * This method works like {@link readAndTrack} but will automatically create the correct selector and comparator for the given keys. The actor will be re-run if any of the values for any of the keys change by strict equality.
   *
   * @param signal - Reference to the signal that should be queried.
   * @param keys - Tuple of keys that should be extracted from the signal.
   * @returns A filtered version of the signal state containing only the requested keys.
   */
  readKeysAndTrack<TState extends object, TKeys extends keyof TState>(
    signal: FactoryOrInstance<ReactiveSource<TState>, TBase>,
    keys: readonly TKeys[],
  ): Pick<TState, TKeys>

  /**
   * Shorthand for reading a reactive value **without** tracking.
   *
   * @remarks
   *
   * Equivalent to `context.reactor.use(signalFactory).read()`.
   *
   * @param signalFactory - The signal to read.
   */
  read<TState>(
    signalFactory: FactoryOrInstance<ReactiveSource<TState>, TBase>,
  ): TState
}
