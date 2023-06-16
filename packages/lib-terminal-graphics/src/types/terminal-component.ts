import { Key } from "node:readline"

import { RenderEnvironment } from "./render-environment"

export type TerminalComponentState = object

export interface StaticTerminalComponent {
  type: "static"
  render(environment: RenderEnvironment): string[]
}

export interface InteractiveTerminalComponent<
  TState extends TerminalComponentState = TerminalComponentState,
  TResult = unknown
> {
  type: "interactive"
  initialState: TState
  isFinal(state: TState): boolean
  update(state: TState, input: Key): TState
  render(state: TState, environment: RenderEnvironment): string[]
  result(state: TState): TResult
}

export type TerminalComponent =
  | StaticTerminalComponent
  | InteractiveTerminalComponent

export type InferComponentResult<TComponent extends TerminalComponent> =
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  TComponent extends InteractiveTerminalComponent<any, infer TResult>
    ? TResult
    : void
