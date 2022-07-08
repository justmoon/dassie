import { createReactor } from "../create-reactor"
import type { Effect } from "../create-reactor"
import { createStore } from "../create-store"
import { createTopic } from "../create-topic"
import type { EffectContext } from "../use-effect"

const topic1 = createTopic<string>("topic1")

const store1 = createStore<{ states: string[] }>("store1", {
  states: [],
})

const rootEffect = (sig: EffectContext) => {
  console.log("root effect created")

  sig.use((sig) => {
    const message = sig.get(topic1)
    console.log("heard", message)
  })

  sig.interval(() => {
    sig.reactor.emit(topic1, "hello" + String(Math.floor(Math.random() * 10)))
  }, 1000)

  sig.onCleanup(() => {
    console.log("root effect cleaned up")
  })

  sig.use(childEffect)
  sig.use(childEffect2)
}

const childEffect: Effect = (sig) => {
  console.log("child effect created")
  const message = sig.get(topic1)

  console.log("reacting to", message)

  if (message) {
    sig.reactor.emit(store1, ({ states }) => ({
      states: [...new Set<string>([...states, message])],
    }))
  }

  sig.onCleanup(() => {
    console.log("child effect cleaned up")
  })
}

const childEffect2: Effect<void> = (sig) => {
  console.log("child effect 2 created")
  const stateCount = sig.get(store1, ({ states }) => states.length)

  console.log(stateCount)

  if (stateCount > 4) {
    console.log("stopping")
    void stopApplication()
  }

  sig.onCleanup(() => {
    console.log("child effect 2 cleaned up")
  })
}

const { dispose: stopApplication } = createReactor(rootEffect)
