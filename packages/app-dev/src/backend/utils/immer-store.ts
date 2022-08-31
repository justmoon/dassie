import produce from "immer"
import type { WritableDraft } from "immer/dist/internal"

import { Store, createStore } from "@dassie/lib-reactive"

export type ImmerProducer<TState> = (
  previousState: WritableDraft<TState>
) => void

export type ImmerStore<TState> = Store<TState, ImmerProducer<TState>>

export const createImmerStore = <TState>(
  initialValue: TState
): ImmerStore<TState> => {
  const store = createStore<TState>(initialValue)
  const reducer =
    (producer: ImmerProducer<TState>) => (previousState: TState) => {
      return produce(previousState, producer)
    }

  const update = (producer: ImmerProducer<TState>) => {
    return store.update(reducer(producer))
  }

  return {
    ...store,
    update,
  }
}
