import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useSyncExternalStore,
} from "react"

import { EffectContext, Reactor, createReactor } from "@dassie/lib-reactive"
import type { LifecycleScope } from "@dassie/lib-reactive/src/internal/lifecycle-scope"

interface ReactEffectContext {
  lifecycle: LifecycleScope
  context: EffectContext
  wake: () => void
}

const noop = () => {
  // empty
}

export const createReactiveHooks = () => {
  const ReactorContext = createContext<Reactor>(createReactor(() => undefined))

  const useReactor = () => useContext(ReactorContext)

  const createReactEffectContext = (reactor: Reactor) => {
    const lifecycle = reactor.deriveChildLifecycle()
    const reactEffectContext: ReactEffectContext = {
      lifecycle,
      context: new EffectContext(
        reactor,
        lifecycle,
        () => {
          reactEffectContext.wake()
        },
        "react"
      ),
      wake: noop,
    }

    return reactEffectContext
  }

  const useSig = () => {
    const reactor = useReactor()

    const effectContext = useRef<ReactEffectContext | undefined>()

    if (effectContext.current === undefined) {
      effectContext.current = createReactEffectContext(reactor)
    }

    useEffect(() => {
      return () => {
        effectContext.current!.lifecycle.dispose().catch((error: unknown) => {
          console.error("error while disposing react effect context", { error })
        })
      }
    }, [reactor])

    return useSyncExternalStore(
      useCallback(
        (listener) => {
          const handleWake = () => {
            effectContext
              .current!.lifecycle.dispose()
              .catch((error: unknown) => {
                console.error("error while disposing react effect context", {
                  error,
                })
              })
              .finally(() => {
                effectContext.current = createReactEffectContext(reactor)
                effectContext.current.wake = handleWake
                listener()
              })
          }

          effectContext.current!.wake = handleWake

          return () => {
            effectContext.current!.wake = noop
          }
        },
        [reactor]
      ),
      () => effectContext.current!.context
    )
  }

  return {
    Provider: ReactorContext.Provider,
    useReactor,
    useSig,
  }
}

const { Provider, useReactor, useSig } = createReactiveHooks()

export { Provider, useReactor, useSig }
