import { Key } from "node:readline"

import type { ReadonlySignal } from "@dassie/lib-reactive"

import { RenderEnvironment } from "./render-environment"

export type TerminalComponentState = object

export interface StaticTerminalComponent {
  type: "static"
  render(environment: RenderEnvironment): string[]
}

export interface DynamicTerminalComponent<
  TState extends TerminalComponentState = TerminalComponentState,
  TSignal extends ReadonlySignal<TState> = ReadonlySignal<TState>,
> {
  type: "dynamic"
  state: TSignal
  refreshInterval: number
  render(
    state: TState,
    isFinal: boolean,
    environment: RenderEnvironment,
  ): string[]
}

export interface InteractiveTerminalComponent<
  TState extends TerminalComponentState = TerminalComponentState,
  TResult = unknown,
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
  | DynamicTerminalComponent
  | InteractiveTerminalComponent

export type InferComponentResult<TComponent extends TerminalComponent> =
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  TComponent extends InteractiveTerminalComponent<any, infer TResult> ? TResult
  : void
