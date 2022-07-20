import type { EffectContext } from "../effect"
import { createReactor } from "../reactor"
import type { Effect } from "../reactor"
import { createStore } from "../store"
import { createTopic } from "../topic"

const topic1 = () => createTopic<string>()

const store1 = () =>
  createStore<{ states: string[] }>({
    states: [],
  })

const rootEffect = (sig: EffectContext) => {
  console.log("root effect created")

  sig.on(topic1, (message) => {
    console.log("heard", message)
  })

  sig.interval(() => {
    sig.emit(topic1, "hello" + String(Math.floor(Math.random() * 10)))
  }, 1000)

  sig.onCleanup(() => {
    console.log("root effect cleaned up")
  })

  sig.use(childEffect)
  sig.use(childEffect2)
}

const childEffect: Effect = (sig) => {
  console.log("child effect created")

  sig.on(topic1, (message) => {
    console.log("reacting to", message)

    if (message) {
      sig.emit(store1, ({ states }) => ({
        states: [...new Set<string>([...states, message])],
      }))
    }
  })

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
    void sig.reactor.dispose()
  }

  sig.onCleanup(() => {
    console.log("child effect 2 cleaned up")
  })
}

createReactor(rootEffect)
