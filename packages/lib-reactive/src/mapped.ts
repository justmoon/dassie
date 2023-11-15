import { isObject } from "@dassie/lib-type-utils"

import { FactoryNameSymbol } from "./internal/context-base"
import {
  Reactive,
  ReactiveSource,
  defaultComparator,
} from "./internal/reactive"
import {
  DisposableLifecycleScope,
  LifecycleScope,
  createLifecycleScope,
} from "./lifecycle"
import { ReadonlyTopic, createTopic } from "./topic"

export const MappedSymbol = Symbol("das:reactive:map")

export interface Mapped<TInput, TOutput> {
  /**
   * Marks this object as a map.
   */
  [MappedSymbol]: true

  /**
   * Name of the map.
   */
  [FactoryNameSymbol]: string

  /**
   * Retrieve an item from the map based on the key.
   *
   * @remarks
   *
   * Return undefined if the item is not currently present in the map.
   */
  get(key: TInput): TOutput | undefined

  /**
   * Retrieve an item from the map along with its lifecycle based on the key.
   *
   * @remarks
   *
   * If the item is not currently present in the map, return undefined for both the item and the lifecycle.
   *
   * @returns A tuple consisting of the item and its lifecycle.
   */
  getWithLifecycle(
    key: TInput,
  ): readonly [TOutput, LifecycleScope] | readonly [undefined, undefined]

  /**
   * Check if the map contains an item with the given key.
   */
  has(key: TInput): boolean

  /**
   * A topic tracking new additions to the map.
   */
  additions: ReadonlyTopic<readonly [TInput, TOutput, LifecycleScope]>

  /**
   * You can iterate over the map. The iterator returns a tuple consisting of the key, the item and its lifecycle.
   */
  [Symbol.iterator]: () => IterableIterator<
    readonly [TInput, TOutput, LifecycleScope]
  >

  /**
   * The number of items in the map.
   */
  size: number
}

interface InternalMapEntry<TOutput> {
  output: TOutput
  lifecycle: DisposableLifecycleScope
}

class MappedImplementation<TInput, TOutput> extends Reactive<
  Map<TInput, InternalMapEntry<TOutput>>
> {
  [MappedSymbol] = true as const;
  [FactoryNameSymbol] = "anonymous"

  private internalMap = new Map<TInput, InternalMapEntry<TOutput>>()
  public readonly additions =
    createTopic<readonly [TInput, TOutput, LifecycleScope]>()

  constructor(
    private parentLifecycle: LifecycleScope,
    private baseSet: ReactiveSource<Set<TInput>>,
    private mapFunction: (input: TInput, lifecycle: LifecycleScope) => TOutput,
  ) {
    super(defaultComparator, true)

    parentLifecycle.onCleanup(() => {
      this.removeParentObservers()
    })
  }

  get(key: TInput) {
    this.read()
    return this.internalMap.get(key)?.output
  }

  getWithLifecycle(key: TInput) {
    this.read()
    const item = this.internalMap.get(key)
    return item
      ? ([item.output, item.lifecycle] as const)
      : ([undefined, undefined] as const)
  }

  has(key: TInput) {
    this.read()
    return this.internalMap.has(key)
  }

  *[Symbol.iterator]() {
    this.read()
    for (const [key, item] of this.internalMap) {
      yield [key, item.output, item.lifecycle] as const
    }
  }

  get size() {
    this.read()
    return this.internalMap.size
  }

  recompute() {
    const newSet = this.readWithTracking(this.baseSet)

    for (const [key, item] of this.internalMap) {
      if (!newSet.has(key)) {
        item.lifecycle.dispose().catch((error: unknown) => {
          console.error("error in map item disposer", {
            map: this[FactoryNameSymbol],
            error,
          })
        })
        this.internalMap.delete(key)
      }
    }

    for (const item of newSet) {
      if (!this.internalMap.has(item)) {
        const lifecycle = createLifecycleScope(
          `${this[FactoryNameSymbol]} item`,
        )
        lifecycle.confineTo(this.parentLifecycle)

        const output = this.mapFunction(item, lifecycle)

        this.internalMap.set(item, {
          output,
          lifecycle,
        })

        this.additions.emit([item, output, lifecycle])
      }
    }

    this.cache = this.internalMap
  }
}

export function createMapped<TInput, TOutput>(
  parentLifecycle: LifecycleScope,
  baseSet: ReactiveSource<Set<TInput>>,
  mapFunction: (input: TInput, lifecycle: LifecycleScope) => TOutput,
): Mapped<TInput, TOutput> {
  return new MappedImplementation(parentLifecycle, baseSet, mapFunction)
}

export const isMapped = (object: unknown): object is Mapped<unknown, unknown> =>
  isObject(object) && object[MappedSymbol] === true
