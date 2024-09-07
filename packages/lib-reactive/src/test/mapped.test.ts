import { enableMapSet, produce } from "immer"
import { describe, test, vi } from "vitest"

import { setTimeout } from "node:timers/promises"

import {
  type ActorContext,
  type Reactor,
  createActor,
  createMapped,
  createReactor,
  createSignal,
} from ".."

enableMapSet()

describe("createMapped", () => {
  test("should be able to create a mapped", ({ expect }) => {
    const reactor = createReactor()
    const signal = createSignal(new Set<void>())
    const mapped = createMapped(reactor, signal, () => {})

    expect(mapped).toBeTypeOf("object")
  })

  test("should instantiate a value for each member of the set", ({
    expect,
  }) => {
    const reactor = createReactor()
    const signal = createSignal(new Set(["A", "B", "C"]))
    const mapped = createMapped(reactor, signal, (key) => key.toLowerCase())

    expect(mapped.get("A")).toBe("a")
    expect(mapped.get("B")).toBe("b")
    expect(mapped.get("C")).toBe("c")
  })

  describe("with runMap", () => {
    test("should run one actor for each member of the set", async ({
      expect,
    }) => {
      const signal = createSignal(new Set(["A", "B", "C"] as const))
      const behaviors = {
        A: vi.fn(() => "a"),
        B: vi.fn(() => "b"),
        C: vi.fn(() => "c"),
      }
      const MappedActors = (reactor: Reactor) =>
        createMapped(reactor, signal, (key) => createActor(behaviors[key]))

      createReactor(() =>
        createActor((sig) => {
          sig.runMap(MappedActors)
        }),
      )

      expect(behaviors.A).toHaveBeenCalledTimes(1)
      expect(behaviors.B).toHaveBeenCalledTimes(1)
      expect(behaviors.C).toHaveBeenCalledTimes(1)

      signal.update(
        produce((draft) => {
          draft.delete("C")
        }),
      )
      await setTimeout(50)

      expect(behaviors.A).toHaveBeenCalledTimes(1)
      expect(behaviors.B).toHaveBeenCalledTimes(1)
      expect(behaviors.C).toHaveBeenCalledTimes(1)
    })

    test("should run cleanup handlers for removed elements", async ({
      expect,
    }) => {
      const signal = createSignal(new Set(["A", "B", "C"] as const))
      const cleanupHandlerC = vi.fn()
      const behaviors = {
        A: () => "a",
        B: () => "b",
        C: (sig: ActorContext) => {
          sig.onCleanup(cleanupHandlerC)
          return "c"
        },
      }
      const MappedActors = (reactor: Reactor) =>
        createMapped(reactor, signal, (key) => createActor(behaviors[key]))

      createReactor(() =>
        createActor((sig) => {
          sig.runMap(MappedActors)
        }),
      )

      expect(cleanupHandlerC).toHaveBeenCalledTimes(0)

      signal.update(
        produce((draft) => {
          draft.delete("C")
        }),
      )

      await setTimeout()

      expect(cleanupHandlerC).toHaveBeenCalledTimes(1)
    })

    test("should run new actors for elements that are added later", async ({
      expect,
    }) => {
      const signal = createSignal(
        new Set(["A", "B", "C"] as ("A" | "B" | "C" | "D")[]),
      )
      const behaviors = {
        A: vi.fn(() => "a"),
        B: vi.fn(() => "b"),
        C: vi.fn(() => "c"),
        D: vi.fn(() => "d"),
      }
      const MappedActors = (reactor: Reactor) =>
        createMapped(reactor, signal, (key) => createActor(behaviors[key]))

      createReactor(() =>
        createActor((sig) => {
          sig.runMap(MappedActors)
        }),
      )

      expect(behaviors.A).toHaveBeenCalledTimes(1)
      expect(behaviors.B).toHaveBeenCalledTimes(1)
      expect(behaviors.C).toHaveBeenCalledTimes(1)
      expect(behaviors.D).toHaveBeenCalledTimes(0)

      signal.update(
        produce((draft) => {
          draft.add("D")
        }),
      )
      await setTimeout()

      expect(behaviors.A).toHaveBeenCalledTimes(1)
      expect(behaviors.B).toHaveBeenCalledTimes(1)
      expect(behaviors.C).toHaveBeenCalledTimes(1)
      expect(behaviors.D).toHaveBeenCalledTimes(1)
    })
  })

  test("should run cleanup handlers for actors corresponding to elements that were added later", async ({
    expect,
  }) => {
    const signal = createSignal(
      new Set(["A", "B", "C"] as ("A" | "B" | "C" | "D")[]),
    )
    const cleanupHandlerD = vi.fn()
    const behaviors = {
      A: () => "a",
      B: () => "b",
      C: () => "c",
      D: (sig: ActorContext) => {
        sig.onCleanup(cleanupHandlerD)
        return "d"
      },
    }
    const MappedActors = (reactor: Reactor) =>
      createMapped(reactor, signal, (key) => createActor(behaviors[key]))

    createReactor(() =>
      createActor((sig) => {
        sig.runMap(MappedActors)
      }),
    )

    expect(cleanupHandlerD).toHaveBeenCalledTimes(0)

    signal.update(
      produce((draft) => {
        draft.add("D")
      }),
    )

    await setTimeout()

    expect(cleanupHandlerD).toHaveBeenCalledTimes(0)

    signal.update(
      produce((draft) => {
        draft.delete("D")
      }),
    )

    await setTimeout()

    expect(cleanupHandlerD).toHaveBeenCalledTimes(1)
  })
})
