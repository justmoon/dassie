import type { ContextState } from "../reactor"
import { isStore } from "../store"

export class DebugTools {
  constructor(readonly contextState: ContextState) {}

  /**
   * Retrieve the state of the reactor.
   *
   * @returns All currently instantiated stores in the reactor.
   */
  getState() {
    return [...this.contextState.values()].filter((possibleStore) =>
      isStore(possibleStore)
    )
  }
}

export const createDebugTools = import.meta.env.DEV
  ? (topicsCache: ContextState) => new DebugTools(topicsCache)
  : () => {
      throw new Error("Debug tools are not available in production.")
    }
