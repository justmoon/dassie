import { createActor } from "../actor"
import { type Reactor, createReactor } from "../reactor"
import { createSignal } from "../signal"
import { createTopic } from "../topic"

const Topic1 = () => createTopic<string>()

const Signal1 = () =>
  createSignal<{ states: string[] }>({
    states: [],
  })

const RootActor = () =>
  createActor((sig) => {
    console.info("root actor created")

    sig.on(Topic1, (message) => {
      console.info("heard", message)
    })

    sig.interval(() => {
      sig.reactor
        .use(Topic1)
        .emit("hello" + String(Math.floor(Math.random() * 10)))
    }, 1000)

    sig.onCleanup(() => {
      console.info("root actor cleaned up")
    })

    sig.run(SubActor)
    sig.run(SubActor2)
  })

const SubActor = () =>
  createActor((sig) => {
    console.info("child actor created")

    sig.on(Topic1, (message) => {
      console.info("reacting to", message)

      if (message) {
        sig.reactor.use(Signal1).update(({ states }) => ({
          states: [...new Set<string>([...states, message])],
        }))
      }
    })

    sig.onCleanup(() => {
      console.info("child actor cleaned up")
    })
  })

const SubActor2 = (reactor: Reactor) =>
  createActor((sig) => {
    console.info("child actor 2 created")
    const stateCount = sig.readAndTrack(Signal1, ({ states }) => states.length)

    console.info(stateCount)

    if (stateCount > 4) {
      console.info("stopping")
      void reactor.dispose()
    }

    sig.onCleanup(() => {
      console.info("child actor 2 cleaned up")
    })
  })

createReactor(RootActor)
