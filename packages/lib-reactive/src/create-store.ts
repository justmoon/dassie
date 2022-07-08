import type { Topic } from "./create-topic"

export type Reducer<TState> = (previousState: TState) => TState

export type Store<TState> = Topic<TState, Reducer<TState>, TState>

export const createStore = <TState>(
  name: string,
  initialValue: TState
): Topic<TState, Reducer<TState>, TState> => {
  // We construct a temporary object in order to assign the name to the function
  // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
  const transformer = {
    [name]: (reducer: Reducer<TState>, previousValue: TState) => {
      return reducer(previousValue)
    },
  }[name]!

  return Object.assign(transformer, { initialValue })
}
