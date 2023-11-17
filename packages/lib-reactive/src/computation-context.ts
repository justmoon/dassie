import { LifecycleScope } from "./lifecycle"
import { ReactiveContextImplementation } from "./reactive-context"
import { ReactiveContext } from "./types/reactive-context"
import { StatefulContext } from "./types/stateful-context"

export interface ComputationContext
  extends StatefulContext,
    ReactiveContext,
    LifecycleScope {}

export class ComputationContextImplementation
  extends ReactiveContextImplementation
  implements ComputationContext {}
