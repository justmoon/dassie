import { Effect, EffectContext, runEffect } from "../effect"
import type { StoreFactory } from "../store"
import { LifecycleScope } from "./lifecycle-scope"

interface EffectCache<TReturn> {
  returnPromise: Promise<TReturn>
  lifecycleScope: LifecycleScope
}

export const createArrayEffect = <
  TElement,
  TReturn,
  TTopicFactory extends StoreFactory<TElement[]>
>(
  arrayTopicFactory: TTopicFactory,
  effect: Effect<TElement, TReturn>,
  parentEffect: Effect<never>
) => {
  return function keepArrayEffectsCache(sig: EffectContext) {
    const runningEffects = new Map<TElement, EffectCache<TReturn>>()

    sig.onCleanup(async () => {
      const disposerPromises: Promise<void>[] = []
      for (const cache of runningEffects.values()) {
        const result = cache.lifecycleScope.dispose()
        if (typeof result.then === "function") disposerPromises.push(result)
      }
      runningEffects.clear()
      await Promise.all(disposerPromises)
    })

    return sig.use(function handleArrayChanges(sig: EffectContext) {
      const arrayValue = sig.get(arrayTopicFactory)

      const asSet = new Set(arrayValue)

      const removed = new Set<TElement>()

      for (const element of runningEffects.keys()) {
        if (!asSet.has(element)) {
          removed.add(element)
        }
      }

      const disposalPromises = Array.from({ length: removed.size })

      let index = 0
      for (const element of removed) {
        const effectCache = runningEffects.get(element)
        disposalPromises[index++] = effectCache!.lifecycleScope.dispose()
        runningEffects.delete(element)
      }

      const disposalPromise = Promise.all(disposalPromises)

      const results: Promise<TReturn>[] = arrayValue.map((element, index) => {
        if (runningEffects.has(element)) {
          return runningEffects.get(element)!.returnPromise
        }

        const lifecycleScope = new LifecycleScope()

        const returnPromise = disposalPromise.then(() => {
          let returnValue!: TReturn

          runEffect(
            sig.reactor,
            effect,
            element,
            lifecycleScope,
            (result) => (returnValue = result)
          ).catch((error: unknown) => {
            console.error("error in array effect", {
              effect: effect.name,
              topic: arrayTopicFactory.name,
              index,
              parentEffect: parentEffect.name,
              error,
            })
          })

          return returnValue
        })

        runningEffects.set(element, {
          returnPromise,
          lifecycleScope,
        })

        return returnPromise
      })

      return results
    })
  }
}
