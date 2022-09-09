import { Patch, enablePatches, produceWithPatches } from "immer"

import { Signal, createSignal } from "@dassie/lib-reactive"

import type { ImmerProducer } from "./immer-store"

type PatchesTuple<TState> = readonly [
  nextState: TState,
  patches: readonly Patch[],
  inversePatches: readonly Patch[]
]

enablePatches()

export const createImmerStoreWithPatches = <TState>(
  initialValue: TState
): Signal<PatchesTuple<TState>, ImmerProducer<TState>> => {
  const signal = createSignal<PatchesTuple<TState>>([
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
    return signal.update(reducer(producer))
  }

  return {
    ...signal,
    update,
  }
}
