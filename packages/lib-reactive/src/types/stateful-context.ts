import type { Reactor } from "../reactor"

export interface StatefulContext<TBase extends object> {
  readonly reactor: Reactor<TBase>
}
