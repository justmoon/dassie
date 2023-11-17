import { createActor } from "../actor"
import { createReactor } from "../reactor"
import { createSignal } from "../signal"

const CounterSignal = () => createSignal(0)

const RootActor = () =>
  createActor((sig) => {
    sig.interval(() => {
      sig.reactor.use(CounterSignal).update((a) => a + 1)
    }, 500)

    void sig.run(InnerActor)
  })

const InnerActor = () =>
  createActor(async (sig) => {
    const counter = sig.readAndTrack(CounterSignal)

    // This will only print every three seconds or so
    console.info(counter)

    // Wait long enough so the counter will have updated a bunch of times before we're ready to run again
    await new Promise((resolve) => setTimeout(resolve, 3000))
  })

createReactor(RootActor)
