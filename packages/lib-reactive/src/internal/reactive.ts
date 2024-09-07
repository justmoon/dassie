import type { Tagged } from "type-fest"

import { ContextBase } from "./context-base"
import { defaultSelector } from "./default-selector"

export const CacheStatus = {
  Clean: 0 as Tagged<0, "StatusClean">,
  Check: 1 as Tagged<1, "StatusCheck">,
  Dirty: 2 as Tagged<2, "StatusDirty">,
} as const

export type CacheStatus = (typeof CacheStatus)[keyof typeof CacheStatus]

export interface ReactiveSource<T> {
  read(): T
  addObserver(consumer: ReactiveObserver): void
  removeObserver(consumer: ReactiveObserver): void
}

export interface ReactiveObserver {
  stale(
    newCacheStatus: typeof CacheStatus.Check | typeof CacheStatus.Dirty,
  ): void
}

export type InferReactiveValue<T> = T extends { read(): infer U } ? U : never

export const defaultComparator = (a: unknown, b: unknown): boolean => a === b

export const CacheUninitialized = Symbol("das:reactive:cache-uninitialized")

export abstract class Reactive<T>
  extends ContextBase
  implements ReactiveSource<T>, ReactiveObserver
{
  private static effectQueue: ReactiveSource<unknown>[] = []
  private static areEffectsScheduled = false

  protected cache: T | typeof CacheUninitialized = CacheUninitialized
  protected cacheStatus: CacheStatus = CacheStatus.Dirty
  private observers = new Set<ReactiveObserver>()
  protected sources = new Set<ReactiveSource<unknown>>()

  protected abstract recompute(): void

  constructor(
    private readonly equals: (a: T, b: T) => boolean = defaultComparator,
    private readonly hasSideEffects = false,
  ) {
    super()
  }

  read(): T {
    this.recomputeIfNecessary()

    if (this.cache === CacheUninitialized) {
      throw new Error(
        "Reactive value should not be uninitialized since we would either have had a cached value (in 'Clean' state) or just calculated one (in other states)",
      )
    }

    return this.cache
  }

  write(value: T): void {
    if (this.cache === CacheUninitialized || !this.equals(this.cache, value)) {
      if (this.observers.size > 0) {
        for (const observer of this.observers) {
          observer.stale(CacheStatus.Dirty)
        }
      }
      this.cache = value
    }

    this.cacheStatus = CacheStatus.Clean
  }

  protected readWithTracking<TState>(source: ReactiveSource<TState>): TState
  protected readWithTracking<TState, TSelection>(
    source: ReactiveSource<TState>,
    selector: (state: TState) => TSelection,
    comparator?: (oldValue: TSelection, newValue: TSelection) => boolean,
  ): TSelection
  protected readWithTracking<TState, TSelection>(
    source: ReactiveSource<TState>,
    selector: (state: TState) => TSelection = defaultSelector as unknown as (
      state: TState,
    ) => TSelection,
    comparator: (
      oldValue: TSelection,
      newValue: TSelection,
    ) => boolean = defaultComparator,
  ) {
    if (selector === defaultSelector && comparator === defaultComparator) {
      this.sources.add(source)
      return source.read()
    } else {
      const intermediateSignal = new ReactiveSelector(
        source,
        selector,
        comparator,
      )
      return this.readWithTracking(intermediateSignal)
    }
  }

  public stale(
    newCacheStatus: typeof CacheStatus.Check | typeof CacheStatus.Dirty,
  ) {
    if (this.cacheStatus < newCacheStatus) {
      if (this.cacheStatus === CacheStatus.Clean && this.hasSideEffects) {
        Reactive.scheduleEffect(this)
      }

      this.cacheStatus = newCacheStatus

      for (const observer of this.observers) {
        observer.stale(CacheStatus.Check)
      }
    }
  }

  public recomputeIfNecessary() {
    if (this.cacheStatus === CacheStatus.Check) {
      for (const source of this.sources) {
        source.read()

        if ((this.cacheStatus as CacheStatus) === CacheStatus.Dirty) {
          break
        }
      }
    }

    if (this.cacheStatus === CacheStatus.Dirty) {
      const previousValue = this.cache
      const previousSources = this.sources
      this.sources = new Set()

      this.recompute()

      // Unsubscribe from any sources that are no longer used.
      for (const source of previousSources) {
        if (!this.sources.has(source)) {
          source.removeObserver(this)
        }
      }

      // Subscribe to any new sources.
      for (const source of this.sources) {
        if (!previousSources.has(source)) {
          source.addObserver(this)
        }
      }

      if (
        previousValue === CacheUninitialized ||
        this.cache === CacheUninitialized ||
        !this.equals(previousValue, this.cache)
      ) {
        for (const observer of this.observers) {
          observer.stale(CacheStatus.Dirty)
        }
      }
    }

    this.cacheStatus = CacheStatus.Clean
  }

  public addObserver(observer: ReactiveObserver) {
    this.observers.add(observer)
  }

  public removeObserver(observer: ReactiveObserver) {
    this.observers.delete(observer)
  }

  public removeParentObservers() {
    for (const source of this.sources) {
      source.removeObserver(this)
    }
  }

  private static scheduleEffect(effect: ReactiveSource<unknown>) {
    Reactive.effectQueue.push(effect)

    if (!Reactive.areEffectsScheduled) {
      Reactive.areEffectsScheduled = true
      queueMicrotask(Reactive.runEffects)
    }
  }

  private static runEffects(this: void) {
    Reactive.areEffectsScheduled = false

    for (const effect of Reactive.effectQueue) {
      effect.read()
    }

    Reactive.effectQueue.length = 0
  }
}

export class ReactiveSelector<TState, TSelection> extends Reactive<TSelection> {
  constructor(
    private readonly source: ReactiveSource<TState>,
    private readonly selector: (state: TState) => TSelection,
    comparator: (oldValue: TSelection, newValue: TSelection) => boolean,
  ) {
    super(comparator, false)
  }

  recompute() {
    this.cache = this.selector(this.readWithTracking(this.source))
  }
}
