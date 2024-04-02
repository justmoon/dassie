import { ReactiveSource } from "./internal/reactive"
import { LifecycleScope } from "./lifecycle"
import { ReactiveContextImplementation } from "./reactive-context"
import { Reactor } from "./reactor"
import {
  LifecycleContext,
  LifecycleContextShortcuts,
} from "./types/lifecycle-context"
import { ReactiveContext } from "./types/reactive-context"
import { StatefulContext } from "./types/stateful-context"

export interface ComputationContext<TBase extends object = object>
  extends StatefulContext<TBase>,
    ReactiveContext<TBase>,
    LifecycleContext,
    LifecycleContextShortcuts {}

export class ComputationContextImplementation<TBase extends object>
  extends ReactiveContextImplementation<TBase>
  implements ComputationContext<TBase>
{
  constructor(
    readonly lifecycle: LifecycleScope,
    reactor: Reactor<TBase>,
    _get: <TState>(signal: ReactiveSource<TState>) => TState,
  ) {
    super(reactor, _get)
  }

  get isDisposed() {
    return this.lifecycle.isDisposed
  }

  get onCleanup() {
    return this.lifecycle.onCleanup
  }

  get offCleanup() {
    return this.lifecycle.offCleanup
  }
}
