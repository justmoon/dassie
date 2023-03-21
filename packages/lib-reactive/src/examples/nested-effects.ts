import { createActor } from "../actor"
import { createReactor } from "../reactor"
import { createSignal } from "../signal"
import { createTopic } from "../topic"

const topic1 = () => createTopic<string>()

const signal1 = () =>
  createSignal<{ states: string[] }>({
    states: [],
  })

const rootActor = () =>
  createActor((sig) => {
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

    sig.run(subActor)
    sig.run(subActor2)
  })

const subActor = () =>
  createActor((sig) => {
    console.log("child effect created")

    sig.on(topic1, (message) => {
      console.log("reacting to", message)

      if (message) {
        sig.use(signal1).update(({ states }) => ({
          states: [...new Set<string>([...states, message])],
        }))
      }
    })

    sig.onCleanup(() => {
      console.log("child effect cleaned up")
    })
  })

const subActor2 = () =>
  createActor((sig) => {
    console.log("child effect 2 created")
    const stateCount = sig.get(signal1, ({ states }) => states.length)

    console.log(stateCount)

    if (stateCount > 4) {
      console.log("stopping")
      void sig.reactor.dispose()
    }

    sig.onCleanup(() => {
      console.log("child effect 2 cleaned up")
    })
  })

createReactor(rootActor)
