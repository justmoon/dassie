import {
  DisposableLifecycleScope,
  Disposer,
  LifecycleScope,
} from "../lifecycle"

export interface LifecycleContext {
  lifecycle: LifecycleScope
}

export interface DisposableLifecycleContext {
  lifecycle: DisposableLifecycleScope
}

export interface LifecycleContextShortcuts {
  isDisposed: boolean
  onCleanup: (cleanupHandler: Disposer) => void
  offCleanup: (cleanupHandler: Disposer) => void
}

export interface DisposableLifecycleContextShortcuts
  extends LifecycleContextShortcuts {
  dispose: () => Promise<void>
  confineTo: (parent: LifecycleScope) => void
}
