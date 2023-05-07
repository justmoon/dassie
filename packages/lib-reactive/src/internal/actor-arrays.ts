import { type Actor } from "../actor"
import type { ActorContext } from "../context"
import type { Factory } from "../reactor"
import type { ReadonlySignal } from "../signal"
import { LifecycleScope } from "./lifecycle-scope"

interface EffectCache<TReturn> {
  returnPromise: Promise<TReturn>
  lifecycleScope: LifecycleScope
}

export const forArrayElement = <TElement, TReturn>(
  sig: ActorContext,
  arraySignalFactory: Factory<ReadonlySignal<readonly TElement[]>>,
  actorFactory: Factory<Actor<TReturn, TElement>>,
  parentEffectName: string
) => {
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

  function handleArrayChanges(arrayValue: readonly TElement[]) {
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
        return sig.run(actorFactory, element, {
          parentLifecycleScope: lifecycleScope,
          additionalDebugData: {
            topic: arraySignalFactory.name,
            index,
            parentEffect: parentEffectName,
          },
        }).result!
      })

      runningEffects.set(element, {
        returnPromise,
        lifecycleScope,
      })

      return returnPromise
    })

    return results
  }

  sig.on(arraySignalFactory, handleArrayChanges)

  return handleArrayChanges(sig.use(arraySignalFactory).read())
}

interface IndexedEffectCache<TElement, TReturn> extends EffectCache<TReturn> {
  element: TElement
}

export const forArrayIndex = <TElement, TReturn>(
  sig: ActorContext,
  arraySignalFactory: Factory<ReadonlySignal<readonly TElement[]>>,
  actorFactory: Factory<
    Actor<TReturn, readonly [element: TElement, index: number]>
  >,
  parentEffectName: string
) => {
  let runningEffects: IndexedEffectCache<TElement, TReturn>[] = []

  sig.onCleanup(async () => {
    const disposerPromises: Promise<void>[] = []
    for (const cache of runningEffects.values()) {
      const result = cache.lifecycleScope.dispose()
      if (typeof result.then === "function") disposerPromises.push(result)
    }
    runningEffects = []
    await Promise.all(disposerPromises)
  })

  function handleArrayChanges(arrayValue: readonly TElement[]) {
    const maxIndex = Math.max(runningEffects.length, arrayValue.length) - 1

    const disposalPromises: Promise<void>[] = []

    for (let index = 0; index <= maxIndex; index++) {
      const effectCache = runningEffects[index]
      if (effectCache && effectCache.element !== arrayValue[index]) {
        disposalPromises.push(effectCache.lifecycleScope.dispose())
        // eslint-disable-next-line @typescript-eslint/no-dynamic-delete
        delete runningEffects[index]
      }
    }

    const disposalPromise = Promise.all(disposalPromises)

    const results: Promise<TReturn>[] = arrayValue.map((element, index) => {
      const cache = runningEffects[index]
      if (cache) {
        return cache.returnPromise
      }

      const lifecycleScope = new LifecycleScope()

      const returnPromise = disposalPromise.then(() => {
        return sig.run(actorFactory, [element, index] as const, {
          parentLifecycleScope: lifecycleScope,
          additionalDebugData: {
            topic: arraySignalFactory.name,
            index,
            parentEffect: parentEffectName,
          },
        }).result!
      })

      runningEffects[index] = {
        element,
        returnPromise,
        lifecycleScope,
      }

      return returnPromise
    })

    return results
  }

  sig.on(arraySignalFactory, handleArrayChanges)

  return handleArrayChanges(sig.use(arraySignalFactory).read())
}
