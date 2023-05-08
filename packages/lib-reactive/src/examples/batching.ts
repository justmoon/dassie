import { createActor } from "../actor"
import { createReactor } from "../reactor"
import { createSignal } from "../signal"

const signal1 = () => createSignal(0)
const signal2 = () => createSignal(0)
const signal3 = () => createSignal(0)

const rootActor = () =>
  createActor((sig) => {
    sig.interval(() => {
      // Even though we are triggering three state updates, the actor will only re-run once
      sig.use(signal1).update((a) => a + 1)
      sig.use(signal2).update((a) => a + 3)
      sig.use(signal3).update((a) => a + 5)
    }, 1000)

    sig.run(subActor)

    // Stop the application after 10 seconds
    sig.timeout(() => void sig.reactor.dispose(), 10_000)
  })

const subActor = () =>
  createActor((sig) => {
    const t1 = sig.get(signal1)
    const t2 = sig.get(signal2)
    const t3 = sig.get(signal3)

    console.log(`actor run with ${t1} ${t2} ${t3}`)
  })

createReactor(rootActor)
