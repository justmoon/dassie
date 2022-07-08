export interface Topic<
  TMessage = unknown,
  TTrigger = never,
  TInitial = unknown
> {
  (input: TTrigger, previousValue: TMessage | TInitial): TMessage
  initialValue: TInitial
}

export const createTopic = <T>(name: string): Topic<T, T, undefined> => {
  // We construct a temporary object in order to assign the name to the function
  // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
  const transformer = { [name]: (message: T) => message }[name]!

  return Object.assign(transformer, { initialValue: undefined })
}
