import { produce } from "immer"
import { afterEach, beforeEach, describe, test, vi } from "vitest"

import { setTimeout } from "node:timers/promises"

import {
  ActorContext,
  createActor,
  createReactor,
  createSignal,
  createTopic,
} from ".."

describe("createActor", () => {
  beforeEach(() => {
    vi.useFakeTimers({ toFake: ["queueMicrotask"] })
  })
  afterEach(() => {
    vi.restoreAllMocks()
  })

  test("should create an actor", ({ expect }) => {
    const actor = createActor(() => ({}))

    expect(actor).toBeTypeOf("object")
  })

  describe("with synchronous behavior", () => {
    test("should re-run when tracked signals change", async ({ expect }) => {
      const reactor = createReactor()
      const signal = createSignal(1)

      const behavior = vi.fn((sig: ActorContext) => sig.get(signal) * 3)

      const actor = createActor(behavior)

      expect(actor.result).toBeUndefined()
      expect(behavior).toHaveBeenCalledTimes(0)

      actor.run(reactor, reactor.lifecycle)
      expect(actor.result).toBe(3)
      expect(behavior).toHaveBeenCalledTimes(1)

      signal.write(2)
      expect(actor.result).toBe(3)
      expect(behavior).toHaveBeenCalledTimes(1)

      vi.runAllTicks()
      await setTimeout()
      expect(actor.result).toBe(6)
      expect(behavior).toHaveBeenCalledTimes(2)

      await reactor.lifecycle.dispose()
    })

    test("should not re-run if a tracked signal changes but the value is the same", async ({
      expect,
    }) => {
      const reactor = createReactor()
      const signal = createSignal(1)

      const behavior = vi.fn((sig: ActorContext) => sig.get(signal) * 3)

      const actor = createActor(behavior)

      expect(actor.result).toBeUndefined()
      expect(behavior).toHaveBeenCalledTimes(0)

      actor.run(reactor, reactor.lifecycle)
      expect(actor.result).toBe(3)
      expect(behavior).toHaveBeenCalledTimes(1)

      signal.write(1)
      expect(actor.result).toBe(3)
      expect(behavior).toHaveBeenCalledTimes(1)

      vi.runAllTicks()
      await setTimeout()
      expect(actor.result).toBe(3)
      expect(behavior).toHaveBeenCalledTimes(1)

      await reactor.lifecycle.dispose()
    })

    test("should re-run if a tracked signal changes even if it changes back to the original value in the same tick", async ({
      expect,
    }) => {
      const reactor = createReactor()
      const signal = createSignal(1)

      const behavior = vi.fn((sig: ActorContext) => sig.get(signal) * 3)

      const actor = createActor(behavior)

      expect(actor.result).toBeUndefined()
      expect(behavior).toHaveBeenCalledTimes(0)

      actor.run(reactor, reactor.lifecycle)
      expect(actor.result).toBe(3)
      expect(behavior).toHaveBeenCalledTimes(1)

      signal.write(2)
      expect(actor.result).toBe(3)
      expect(behavior).toHaveBeenCalledTimes(1)

      signal.write(1)
      expect(actor.result).toBe(3)
      expect(behavior).toHaveBeenCalledTimes(1)

      vi.runAllTicks()
      await setTimeout()
      expect(actor.result).toBe(3)
      expect(behavior).toHaveBeenCalledTimes(2)

      await reactor.lifecycle.dispose()
    })

    test("should call cleanup handler if reactor is disposed", async ({
      expect,
    }) => {
      const reactor = createReactor()

      const cleanup = vi.fn(() => {})
      const behavior = vi.fn((sig: ActorContext) => {
        sig.onCleanup(cleanup)
      })

      const actor = createActor(behavior)

      actor.run(reactor, reactor.lifecycle)
      expect(behavior).toHaveBeenCalledTimes(1)
      expect(cleanup).toHaveBeenCalledTimes(0)

      await reactor.lifecycle.dispose()
      expect(cleanup).toHaveBeenCalledTimes(1)
    })

    test("should call cleanup handler if actor re-runs", async ({ expect }) => {
      const reactor = createReactor()
      const signal = createSignal(1)

      const cleanup = vi.fn(() => {})

      const behavior = vi.fn((sig: ActorContext) => {
        sig.get(signal)
        sig.onCleanup(cleanup)
      })

      const actor = createActor(behavior)

      actor.run(reactor, reactor.lifecycle)
      expect(behavior).toHaveBeenCalledTimes(1)
      expect(cleanup).toHaveBeenCalledTimes(0)

      signal.write(2)
      vi.runAllTicks()
      await setTimeout()
      expect(behavior).toHaveBeenCalledTimes(2)
      expect(cleanup).toHaveBeenCalledTimes(1)
    })

    test("should re-run actors that read another actor's result", async ({
      expect,
    }) => {
      const signal = createSignal(1)

      const behaviorA = vi.fn((sig: ActorContext) => sig.get(signal) * 3)
      const ActorA = () => createActor(behaviorA)

      const behaviorB = vi.fn((sig: ActorContext) => {
        const value = sig.get(ActorA)
        return value === undefined ? "foo" : value * 2
      })
      const ActorB = () => createActor(behaviorB)

      const RootActor = () =>
        createActor((sig) => {
          sig.run(ActorA)
          sig.run(ActorB)
        })

      const reactor = createReactor(RootActor)

      expect(behaviorA).toHaveBeenCalledTimes(1)
      expect(behaviorB).toHaveBeenCalledTimes(1)
      // When we synchronously access an actor's result, it will be undefined,
      // because the actor only runs on the next tick.
      expect(reactor.use(ActorB).result).toBe("foo")

      await setTimeout()
      expect(behaviorA).toHaveBeenCalledTimes(1)
      expect(behaviorB).toHaveBeenCalledTimes(2)
      expect(reactor.use(ActorB).result).toBe(6)

      signal.write(2)
      await setTimeout()

      expect(behaviorA).toHaveBeenCalledTimes(2)
      expect(behaviorB).toHaveBeenCalledTimes(3)
      expect(reactor.use(ActorB).result).toBe(12)
    })

    test("should only re-run actor if they selected data changes", async ({
      expect,
    }) => {
      const signal = createSignal({ a: 1, b: 2 })

      const behavior = vi.fn(
        (sig: ActorContext) => sig.get(signal, ({ a }) => a) * 3,
      )

      const Actor = () => createActor(behavior)

      const RootActor = () =>
        createActor((sig) => {
          sig.run(Actor)
        })

      createReactor(RootActor)

      expect(behavior).toHaveBeenCalledTimes(1)

      signal.update(
        produce((draft) => {
          draft.b = 3
        }),
      )

      await setTimeout()

      expect(behavior).toHaveBeenCalledTimes(1)

      signal.update(
        produce((draft) => {
          draft.a = 2
        }),
      )

      await setTimeout()

      expect(behavior).toHaveBeenCalledTimes(2)
    })

    test("should only re-run actor if selected keys change", async ({
      expect,
    }) => {
      const signal = createSignal({ a: 1, b: 2 })

      const behavior = vi.fn((sig: ActorContext) => {
        const { a } = sig.getKeys(signal, ["a"])

        return a * 3
      })

      const Actor = () => createActor(behavior)

      const RootActor = () =>
        createActor((sig) => {
          sig.run(Actor)
        })

      createReactor(RootActor)

      expect(behavior).toHaveBeenCalledTimes(1)

      signal.update(
        produce((draft) => {
          draft.b = 3
        }),
      )

      await setTimeout()

      expect(behavior).toHaveBeenCalledTimes(1)

      signal.update(
        produce((draft) => {
          draft.a = 2
        }),
      )

      await setTimeout()

      expect(behavior).toHaveBeenCalledTimes(2)
    })

    test("should re-run actor on subscribed topic message", async ({
      expect,
    }) => {
      const topic = createTopic<void>()

      const behavior = vi.fn((sig: ActorContext) => {
        sig.subscribe(topic)
      })

      const Actor = () => createActor(behavior)

      createReactor(Actor)

      expect(behavior).toHaveBeenCalledTimes(1)

      topic.emit()

      await setTimeout()

      expect(behavior).toHaveBeenCalledTimes(2)
    })
  })
})
