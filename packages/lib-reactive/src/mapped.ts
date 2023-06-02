import { isObject } from "@dassie/lib-type-utils"

import { DisposableLifecycle, Lifecycle } from "./internal/lifecycle"
import { Factory, FactoryNameSymbol, Reactor } from "./reactor"
import { type ReadonlySignal } from "./signal"
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
    key: TInput
  ): readonly [TOutput, Lifecycle] | readonly [undefined, undefined]

  /**
   * Check if the map contains an item with the given key.
   */
  has(key: TInput): boolean

  /**
   * A topic tracking new additions to the map.
   */
  additions: ReadonlyTopic<readonly [TInput, TOutput, Lifecycle]>

  /**
   * You can iterate over the map. The iterator returns a tuple consisting of the key, the item and its lifecycle.
   */
  [Symbol.iterator]: () => IterableIterator<
    readonly [TInput, TOutput, Lifecycle]
  >
}

interface InternalMapEntry<TOutput> {
  output: TOutput
  lifecycle: DisposableLifecycle
}

export function createMapped<TInput, TOutput>(
  baseSetFactory: Factory<ReadonlySignal<Set<TInput>>>,
  mapFunction: (input: TInput, lifecycle: Lifecycle) => TOutput
): Mapped<TInput, TOutput> {
  const reactor = Reactor.current

  if (!reactor) {
    throw new Error("Maps must be created by a reactor.")
  }

  const baseSet = reactor.use(baseSetFactory)

  const internalMap = new Map<TInput, InternalMapEntry<TOutput>>()

  const additions = createTopic<readonly [TInput, TOutput, Lifecycle]>()

  const mapped: Mapped<TInput, TOutput> = {
    [MappedSymbol]: true,
    [FactoryNameSymbol]: "anonymous",

    get(key) {
      return internalMap.get(key)?.output
    },

    getWithLifecycle(key) {
      const item = internalMap.get(key)
      return item ? [item.output, item.lifecycle] : [undefined, undefined]
    },

    has(key) {
      return internalMap.has(key)
    },

    additions,

    *[Symbol.iterator]() {
      for (const [key, item] of internalMap) {
        yield [key, item.output, item.lifecycle] as const
      }
    },
  }

  for (const item of baseSet.read()) {
    const lifecycle = new DisposableLifecycle(
      `${mapped[FactoryNameSymbol]} item}`
    )
    lifecycle.attachToParent(reactor)

    internalMap.set(item, {
      output: mapFunction(item, lifecycle),
      lifecycle,
    })

    // No need to emit anything on the additions topic, since nobody has had a chance to subscribe yet.
  }

  baseSet.on(reactor, (newSet) => {
    for (const [key, item] of internalMap) {
      if (!newSet.has(key)) {
        item.lifecycle.dispose().catch((error: unknown) => {
          console.error("error in map item disposer", {
            map: mapped[FactoryNameSymbol],
            error,
          })
        })
        internalMap.delete(key)
      }
    }

    for (const item of newSet) {
      if (!internalMap.has(item)) {
        const lifecycle = new DisposableLifecycle(
          `${mapped[FactoryNameSymbol]} item}`
        )
        lifecycle.attachToParent(reactor)

        Reactor.current = reactor
        const output = mapFunction(item, lifecycle)
        Reactor.current = undefined

        internalMap.set(item, {
          output,
          lifecycle,
        })

        additions.emit([item, output, lifecycle])
      }
    }
  })

  return mapped
}

export const isMapped = (object: unknown): object is Mapped<unknown, unknown> =>
  isObject(object) && object[MappedSymbol] === true
