import { createActor } from "../actor"
import { Reactor, createReactor } from "../reactor"
import { createSignal } from "../signal"

const Signal1 = () => createSignal(0)
const Signal2 = () => createSignal(0)
const Signal3 = () => createSignal(0)

const RootActor = (reactor: Reactor) =>
  createActor((sig) => {
    sig.interval(() => {
      // Even though we are triggering three state updates, the actor will only re-run once
      sig.use(Signal1).update((a) => a + 1)
      sig.use(Signal2).update((a) => a + 3)
      sig.use(Signal3).update((a) => a + 5)
    }, 1000)

    sig.run(SubActor)

    // Stop the application after 10 seconds
    sig.timeout(() => void reactor.lifecycle.dispose(), 10_000)
  })

const SubActor = () =>
  createActor((sig) => {
    const t1 = sig.get(Signal1)
    const t2 = sig.get(Signal2)
    const t3 = sig.get(Signal3)

    console.info(`actor run with ${t1} ${t2} ${t3}`)
  })

createReactor(RootActor)
