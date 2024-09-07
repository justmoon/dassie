import type { Reactor } from "../reactor"

/**
 * The reactor will automatically set this property on each instantiated object.
 *
 * @remarks
 *
 * This is a bit of a hack, but it allows our users' code to be cleaner. Specifically, they don't have to repeat the name of the topic, they can just do this and the name `myTopic` will automatically be captured:
 *
 * @example
 *
 * ```ts
 * const myTopic = () => reactor.createTopic<string>()
 * ```
 */
export const FactoryNameSymbol = Symbol("das:reactive:factory-name")

/**
 * Can be used to add a function that will be automatically called after a context value has been instantiated.
 */
export const InitSymbol = Symbol("das:reactive:init")

/**
 * Can be used to add a function that will be automatically called each time a new reference to a context value is created.
 */
export const UseSymbol = Symbol("das:reactive:use")

/**
 * Can be used to add a function that will be automatically called each time a reference to a context value is destroyed.
 */
export const DisposeSymbol = Symbol("das:reactive:dispose")

/**
 * Interface for reactor context values.
 */
export interface ContextValue {
  /**
   * Name of the factory function that created this value.
   *
   * @remarks
   *
   * This field will be assigned by the reactor after the value is instantiated.
   *
   * @see {@link FactoryNameSymbol}
   */
  [FactoryNameSymbol]: string | undefined

  [InitSymbol]?(reactor: Reactor): void
  [UseSymbol]?(reactor: Reactor): void
  [DisposeSymbol]?(reactor: Reactor): void
}

/**
 * Base class for any values that are stored in the reactor context.
 */
export class ContextBase implements ContextValue {
  [FactoryNameSymbol] = "anonymous"
}
