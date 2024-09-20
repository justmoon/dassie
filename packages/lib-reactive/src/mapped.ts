import { isObject } from "@dassie/lib-type-utils"

import { FactoryNameSymbol } from "./internal/context-base"
import {
  Reactive,
  type ReactiveSource,
  defaultComparator,
} from "./internal/reactive"
import {
  type DisposableScope,
  type Scope,
  confineScope,
  createScope,
} from "./scope"
import { type ReadonlyTopic, createTopic } from "./topic"
import type { FactoryOrInstance } from "./types/factory"
import type { ScopeContext } from "./types/scope-context"
import type { StatefulContext } from "./types/stateful-context"

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
   * Retrieve an item from the map along with its scope based on the key.
   *
   * @remarks
   *
   * If the item is not currently present in the map, return undefined for both the item and the scope.
   *
   * @returns A tuple consisting of the item and its scope.
   */
  getWithScope(
    key: TInput,
  ): readonly [TOutput, Scope] | readonly [undefined, undefined]

  /**
   * Check if the map contains an item with the given key.
   */
  has(key: TInput): boolean

  /**
   * A topic tracking new additions to the map.
   */
  additions: ReadonlyTopic<readonly [TInput, TOutput, Scope]>

  /**
   * You can iterate over the map. The iterator returns a tuple consisting of the key, the item and its scope.
   */
  [Symbol.iterator]: () => IterableIterator<readonly [TInput, TOutput, Scope]>

  /**
   * The number of items in the map.
   */
  size: number
}

interface InternalMapEntry<TOutput> {
  output: TOutput
  scope: DisposableScope
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
  [MappedSymbol] = true as const

  private baseSet: ReactiveSource<Set<TInput>>
  private internalMap = new Map<TInput, InternalMapEntry<TOutput>>()
  public readonly additions = createTopic<readonly [TInput, TOutput, Scope]>()
  private nextUniqueId = 0

  constructor(
    private parentContext: StatefulContext<TBase> & ScopeContext,
    baseSetFactory: FactoryOrInstance<ReactiveSource<Set<TInput>>, TBase>,
    private mapFunction: (input: TInput, scope: Scope) => TOutput,
  ) {
    super(defaultComparator, true)

    parentContext.scope.onCleanup(() => {
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

  getWithScope(key: TInput) {
    this.read()
    const item = this.internalMap.get(key)
    return item ?
        ([item.output, item.scope] as const)
      : ([undefined, undefined] as const)
  }

  has(key: TInput) {
    this.read()
    return this.internalMap.has(key)
  }

  *[Symbol.iterator]() {
    this.read()
    for (const [key, item] of this.internalMap) {
      yield [key, item.output, item.scope] as const
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
        item.scope.dispose().catch((error: unknown) => {
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
        const scope = createScope(itemName)
        confineScope(scope, this.parentContext.scope)

        const output = this.mapFunction(key, scope)

        this.internalMap.set(key, {
          output,
          scope,
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

        this.additions.emit([key, output, scope])
      }
    }

    this.cache = this.internalMap
  }
}

export function createMapped<TInput, TOutput, TBase extends object>(
  parentContext: StatefulContext<TBase> & ScopeContext,
  baseSet: FactoryOrInstance<ReactiveSource<Set<TInput>>, TBase>,
  mapFunction: (input: TInput, scope: Scope) => TOutput,
): Mapped<TInput, TOutput> {
  return new MappedImplementation(parentContext, baseSet, mapFunction)
}

export const isMapped = (object: unknown): object is Mapped<unknown, unknown> =>
  isObject(object) && object[MappedSymbol] === true
