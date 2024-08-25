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
import { Factory } from "./types/factory"
import { LifecycleContext } from "./types/lifecycle-context"
import { StatefulContext } from "./types/stateful-context"

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

/**
 * In order to facilitate debuggability, we use string keys in the debug names of instantiated values - up to a maximum
 * length.
 */
const MAX_DEBUG_STRING_KEY_LENGTH = 64

class MappedImplementation<
  TInput,
  TOutput,
  TBase extends object,
> extends Reactive<Map<TInput, InternalMapEntry<TOutput>>> {
  [MappedSymbol] = true as const;
  [FactoryNameSymbol] = "anonymous"

  private baseSet: ReactiveSource<Set<TInput>>
  private internalMap = new Map<TInput, InternalMapEntry<TOutput>>()
  public readonly additions =
    createTopic<readonly [TInput, TOutput, LifecycleScope]>()
  private nextUniqueId = 0

  constructor(
    private parentContext: StatefulContext<TBase> & LifecycleContext,
    baseSetFactory:
      | Factory<ReactiveSource<Set<TInput>>, TBase>
      | ReactiveSource<Set<TInput>>,
    private mapFunction: (input: TInput, lifecycle: LifecycleScope) => TOutput,
  ) {
    super(defaultComparator, true)

    parentContext.lifecycle.onCleanup(() => {
      this.removeParentObservers()
    })

    this.baseSet =
      typeof baseSetFactory === "function" ?
        parentContext.reactor.use(baseSetFactory)
      : baseSetFactory
  }

  get(key: TInput) {
    this.read()
    return this.internalMap.get(key)?.output
  }

  getWithLifecycle(key: TInput) {
    this.read()
    const item = this.internalMap.get(key)
    return item ?
        ([item.output, item.lifecycle] as const)
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

    for (const key of newSet) {
      if (!this.internalMap.has(key)) {
        const itemName = `${this[FactoryNameSymbol]}[${
          typeof key === "string" && key.length < MAX_DEBUG_STRING_KEY_LENGTH ?
            key
          : this.nextUniqueId++
        }]`
        const lifecycle = createLifecycleScope(itemName)
        lifecycle.confineTo(this.parentContext.lifecycle)

        const output = this.mapFunction(key, lifecycle)

        this.internalMap.set(key, {
          output,
          lifecycle,
        })

        // Tag with factory name in debug mode
        if (this.parentContext.reactor.debug && isObject(output)) {
          Object.defineProperty(output, FactoryNameSymbol, {
            value: itemName,
            enumerable: false,
            writable: true,
          })

          this.parentContext.reactor.debug.notifyOfMappedInstantiation(
            output,
            this,
          )
        }

        this.additions.emit([key, output, lifecycle])
      }
    }

    this.cache = this.internalMap
  }
}

export function createMapped<TInput, TOutput, TBase extends object>(
  parentContext: StatefulContext<TBase> & LifecycleContext,
  baseSet:
    | Factory<ReactiveSource<Set<TInput>>, TBase>
    | ReactiveSource<Set<TInput>>,
  mapFunction: (input: TInput, lifecycle: LifecycleScope) => TOutput,
): Mapped<TInput, TOutput> {
  return new MappedImplementation(parentContext, baseSet, mapFunction)
}

export const isMapped = (object: unknown): object is Mapped<unknown, unknown> =>
  isObject(object) && object[MappedSymbol] === true
