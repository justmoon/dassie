import produce from "immer"
import type { WritableDraft } from "immer/dist/internal"

import type { Topic } from "@xen-ilp/lib-reactive"

export type ImmerProducer<TState> = (
  previousState: WritableDraft<TState>
) => void

export type ImmerStore<TState> = Topic<TState, ImmerProducer<TState>, TState>

export const createImmerStore = <TState>(
  name: string,
  initialValue: TState
): Topic<TState, ImmerProducer<TState>, TState> => {
  // We construct a temporary object in order to assign the name to the function
  // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
  const transformer = {
    [name]: (reducer: ImmerProducer<TState>, previousValue: TState) => {
      return produce(previousValue, reducer)
    },
  }[name]!

  return Object.assign(transformer, { initialValue })
}
