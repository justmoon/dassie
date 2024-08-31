import { ReactiveSource } from "./internal/reactive"
import { ReactiveContextImplementation } from "./reactive-context"
import { Reactor } from "./reactor"
import { Scope } from "./scope"
import { ReactiveContext } from "./types/reactive-context"
import { ScopeContext, ScopeContextShortcuts } from "./types/scope-context"
import { StatefulContext } from "./types/stateful-context"

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
