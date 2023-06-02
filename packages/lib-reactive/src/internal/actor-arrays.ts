import { type Actor } from "../actor"
import type { ActorContext } from "../context"
import type { Factory } from "../reactor"
import type { ReadonlySignal } from "../signal"
import { DisposableLifecycle } from "./lifecycle"

interface EffectCache<TReturn> {
  returnPromise: Promise<TReturn | undefined>
  lifecycleScope: DisposableLifecycle
}

export const forArrayElement = <TElement, TReturn>(
  sig: ActorContext,
  arraySignalFactory: Factory<ReadonlySignal<readonly TElement[]>>,
  actorFactory: Factory<Actor<TReturn, TElement>>,
  parentEffectName: string
) => {
  const runningActors = new Map<TElement, EffectCache<TReturn>>()

  sig.onCleanup(async () => {
    const disposerPromises: Promise<void>[] = []
    for (const cache of runningActors.values()) {
      const result = cache.lifecycleScope.dispose()
      if (typeof result.then === "function") disposerPromises.push(result)
    }
    runningActors.clear()
    await Promise.all(disposerPromises)
  })

  function handleArrayChanges(arrayValue: readonly TElement[]) {
    const asSet = new Set(arrayValue)

    const removed = new Set<TElement>()

    for (const element of runningActors.keys()) {
      if (!asSet.has(element)) {
        removed.add(element)
      }
    }

    const disposalPromises = Array.from({ length: removed.size })

    let index = 0
    for (const element of removed) {
      const actorCache = runningActors.get(element)
      disposalPromises[index++] = actorCache!.lifecycleScope.dispose()
      runningActors.delete(element)
    }

    const disposalPromise = Promise.all(disposalPromises)

    const results: Promise<TReturn | undefined>[] = arrayValue.map(
      (element, index) => {
        if (runningActors.has(element)) {
          return runningActors.get(element)!.returnPromise
        }

        const lifecycleScope = new DisposableLifecycle(
          `${parentEffectName}/${actorFactory.name}#${index}`
        )

        const returnPromise = disposalPromise.then(() => {
          return sig
            .use(actorFactory, { stateless: true })
            .run(lifecycleScope, element, {
              additionalDebugData: {
                topic: arraySignalFactory.name,
                index,
                parentEffect: parentEffectName,
              },
            })
        })

        runningActors.set(element, {
          returnPromise,
          lifecycleScope,
        })

        return returnPromise
      }
    )

    return results
  }

  sig.on(arraySignalFactory, (value) => void handleArrayChanges(value))

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
      const actorCache = runningEffects[index]
      if (actorCache && actorCache.element !== arrayValue[index]) {
        disposalPromises.push(actorCache.lifecycleScope.dispose())
        // eslint-disable-next-line @typescript-eslint/no-dynamic-delete
        delete runningEffects[index]
      }
    }

    const disposalPromise = Promise.all(disposalPromises)

    const results: Promise<TReturn | undefined>[] = arrayValue.map(
      (element, index) => {
        const cache = runningEffects[index]
        if (cache) {
          return cache.returnPromise
        }

        const lifecycleScope = new DisposableLifecycle(
          `${parentEffectName}/${actorFactory.name}#${index}`
        )

        const returnPromise = disposalPromise.then(() => {
          return sig
            .use(actorFactory, { stateless: true })
            .run(lifecycleScope, [element, index] as const, {
              additionalDebugData: {
                topic: arraySignalFactory.name,
                index,
                parentEffect: parentEffectName,
              },
            })
        })

        runningEffects[index] = {
          element,
          returnPromise,
          lifecycleScope,
        }

        return returnPromise
      }
    )

    return results
  }

  sig.on(arraySignalFactory, (value) => void handleArrayChanges(value))

  return handleArrayChanges(sig.use(arraySignalFactory).read())
}
