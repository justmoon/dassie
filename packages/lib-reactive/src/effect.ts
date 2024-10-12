import { type ComputationContext } from "./computation-context"
import { createComputed } from "./computed"
import type { ScopeContext } from "./types/scope-context"
import type { StatefulContext } from "./types/stateful-context"

export type EffectContext<TBase extends object = object> =
  ComputationContext<TBase>

export const createEffect = <TBase extends object = object>(
  context: ScopeContext & StatefulContext<TBase>,
  behavior: (sig: EffectContext) => void,
): void => {
  createComputed(context, behavior, { hasSideEffects: true })
}
