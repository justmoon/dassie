import { Patch, enablePatches, produceWithPatches } from "immer"

import type { Topic } from "@xen-ilp/lib-reactive"

import type { ImmerProducer } from "./immer-store"

type PatchesTuple<TState> = readonly [
  nextState: TState,
  patches: readonly Patch[],
  inversePatches: readonly Patch[]
]

enablePatches()

export const createImmerStoreWithPatches = <TState>(
  name: string,
  initialValue: TState
): Topic<PatchesTuple<TState>, ImmerProducer<TState>, PatchesTuple<TState>> => {
  // We construct a temporary object in order to assign the name to the function
  // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
  const transformer = {
    [name]: (
      reducer: ImmerProducer<TState>,
      previousValue: PatchesTuple<TState>
    ) => {
      return produceWithPatches(previousValue[0], reducer)
    },
  }[name]!

  return Object.assign(transformer, {
    initialValue: [initialValue, [], []] as const,
  })
}
