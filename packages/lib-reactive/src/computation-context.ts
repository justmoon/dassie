import { LifecycleScope } from "./lifecycle"
import { ReactiveContextImplementation } from "./reactive-context"
import { ReactiveContext } from "./types/reactive-context"
import { StatefulContext } from "./types/stateful-context"

export interface ComputationContext<TBase extends object = object>
  extends StatefulContext<TBase>,
    ReactiveContext,
    LifecycleScope {}

export class ComputationContextImplementation<TBase extends object>
  extends ReactiveContextImplementation<TBase>
  implements ComputationContext<TBase> {}
