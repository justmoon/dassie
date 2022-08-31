import { Patch, enablePatches, produceWithPatches } from "immer"

import { Store, createStore } from "@dassie/lib-reactive"

import type { ImmerProducer } from "./immer-store"

type PatchesTuple<TState> = readonly [
  nextState: TState,
  patches: readonly Patch[],
  inversePatches: readonly Patch[]
]

enablePatches()

export const createImmerStoreWithPatches = <TState>(
  initialValue: TState
): Store<PatchesTuple<TState>, ImmerProducer<TState>> => {
  const store = createStore<PatchesTuple<TState>>([
    initialValue,
    [],
    [],
  ] as const)

  const reducer =
    (producer: ImmerProducer<TState>) =>
    (previousState: PatchesTuple<TState>) => {
      return produceWithPatches(previousState[0], producer)
    }

  const update = (producer: ImmerProducer<TState>) => {
    return store.update(reducer(producer))
  }

  return {
    ...store,
    update,
  }
}
