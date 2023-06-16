export const submit = <TState extends object>(
  state: TState
): TState & { state: "confirm" } => ({
  ...state,
  state: "confirm",
})

export const cancel = <TState extends object>(
  state: TState
): TState & { state: "cancel" } => ({
  ...state,
  state: "cancel",
})
