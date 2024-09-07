import type { DisposableScope, Disposer, Scope } from "../scope"

export interface ScopeContext {
  scope: Scope
}

export interface DisposableScopeContext {
  scope: DisposableScope
}

export interface ScopeContextShortcuts {
  isDisposed: boolean
  onCleanup: (cleanupHandler: Disposer) => void
  offCleanup: (cleanupHandler: Disposer) => void
}

export interface DisposableScopeContextShortcuts extends ScopeContextShortcuts {
  dispose: () => Promise<void>
}
