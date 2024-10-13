/* eslint-disable react-hooks/exhaustive-deps */

/**
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * Ported to TypeScript by Stefan Thomas.
 *
 * This source code is licensed under the MIT license found in the
 * LICENSE file in the root directory of this source tree.
 */
import {
  useDebugValue,
  useEffect,
  useMemo,
  useRef,
  useSyncExternalStore,
} from "react"

// Same as useSyncExternalStore, but supports selector and isEqual arguments.
export function useSyncExternalStoreWithSelector<TValue, TSelection>(
  subscribe: (onStateChange: () => void) => () => void,
  getSnapshot: () => TValue,
  getServerSnapshot: undefined | null | (() => TValue),
  selector: (snapshot: TValue) => TSelection,
  isEqual?: (a: TSelection, b: TSelection) => boolean,
): TSelection {
  // Use this to track the rendered snapshot.
  const renderedSnapshotReference = useRef<
    | {
        hasValue: true
        value: TSelection
      }
    | {
        hasValue: false
        value: null
      }
    | null
  >(null)

  let inst
  if (renderedSnapshotReference.current === null) {
    inst = {
      hasValue: false as const,
      value: null,
    }
    renderedSnapshotReference.current = inst
  } else {
    inst = renderedSnapshotReference.current
  }

  const [getSelection, getServerSelection] = useMemo(() => {
    // Track the memoized state using a closure variable that is local to this
    // memoized instance of a getSnapshot function. Intentionally not using a
    // useRef hook, because that state would be shared across all concurrent
    // copies of the hook/component.
    let memo:
      | {
          snapshot: TValue
          selection: TSelection
        }
      | undefined

    const memoizedSelector = (nextSnapshot: TValue) => {
      // The first time the hook is called, there is no memoized result.
      if (!memo) {
        const nextSelection = selector(nextSnapshot)

        // Even if the selector has changed, the currently rendered selection
        // may be equal to the new selection. We should attempt to reuse the
        // current value if possible, to preserve downstream memoizations.
        if (isEqual !== undefined && inst.hasValue) {
          const currentSelection = inst.value
          if (isEqual(currentSelection, nextSelection)) {
            memo = {
              snapshot: nextSnapshot,
              selection: currentSelection,
            }
            return currentSelection
          }
        }
        memo = {
          snapshot: nextSnapshot,
          selection: nextSelection,
        }
        return nextSelection
      }

      // We may be able to reuse the previous invocation's result.
      const previousSnapshot: TValue = memo.snapshot
      const previousSelection: TSelection = memo.selection

      if (Object.is(previousSnapshot, nextSnapshot)) {
        // The snapshot is the same as last time. Reuse the previous selection.
        return previousSelection
      }

      // The snapshot has changed, so we need to compute a new selection.
      const nextSelection = selector(nextSnapshot)

      // If a custom isEqual function is provided, use that to check if the data
      // has changed. If it hasn't, return the previous selection. That signals
      // to React that the selections are conceptually equal, and we can bail
      // out of rendering.
      if (isEqual?.(previousSelection, nextSelection)) {
        // The snapshot still has changed, so make sure to update to not keep
        // old references alive
        memo = {
          snapshot: nextSnapshot,
          selection: previousSelection,
        }

        return previousSelection
      }

      memo = {
        snapshot: nextSnapshot,
        selection: nextSelection,
      }
      return nextSelection
    }

    const getSnapshotWithSelector = () => memoizedSelector(getSnapshot())
    const getServerSnapshotWithSelector =
      getServerSnapshot ?
        () => memoizedSelector(getServerSnapshot())
      : undefined
    return [getSnapshotWithSelector, getServerSnapshotWithSelector]
  }, [getSnapshot, getServerSnapshot, selector, isEqual])

  const value = useSyncExternalStore(
    subscribe,
    getSelection,
    getServerSelection,
  )

  useEffect(() => {
    inst.hasValue = true
    inst.value = value
  }, [value])

  useDebugValue(value)

  return value
}
