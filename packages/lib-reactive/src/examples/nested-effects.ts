import type { EffectContext } from "../effect"
import { createReactor } from "../reactor"
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
    sig.use(topic1).emit("hello" + String(Math.floor(Math.random() * 10)))
  }, 1000)

  sig.onCleanup(() => {
    console.log("root effect cleaned up")
  })

  sig.run(childEffect)
  sig.run(childEffect2)
}

const childEffect = (sig: EffectContext) => {
  console.log("child effect created")

  sig.on(topic1, (message) => {
    console.log("reacting to", message)

    if (message) {
      sig.use(store1).update(({ states }) => ({
        states: [...new Set<string>([...states, message])],
      }))
    }
  })

  sig.onCleanup(() => {
    console.log("child effect cleaned up")
  })
}

const childEffect2 = (sig: EffectContext) => {
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
