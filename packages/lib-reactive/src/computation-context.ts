import type { ReactiveSource } from "./internal/reactive"
import { ReactiveContextImplementation } from "./reactive-context"
import type { Reactor } from "./reactor"
import type { Scope } from "./scope"
import type { ReactiveContext } from "./types/reactive-context"
import type { ScopeContext, ScopeContextShortcuts } from "./types/scope-context"
import type { StatefulContext } from "./types/stateful-context"

export interface ComputationContext<TBase extends object = object>
  extends StatefulContext<TBase>,
    ReactiveContext<TBase>,
    ScopeContext,
    ScopeContextShortcuts {}

export class ComputationContextImplementation<TBase extends object>
  extends ReactiveContextImplementation<TBase>
  implements ComputationContext<TBase>
{
  constructor(
    readonly scope: Scope,
    reactor: Reactor<TBase>,
    _get: <TState>(signal: ReactiveSource<TState>) => TState,
  ) {
    super(reactor, _get)
  }

  get isDisposed() {
    return this.scope.isDisposed
  }

  get onCleanup() {
    return this.scope.onCleanup
  }

  get offCleanup() {
    return this.scope.offCleanup
  }
}
