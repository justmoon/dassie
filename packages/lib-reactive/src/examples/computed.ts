import { createActor } from "../actor"
import { createComputed } from "../computed"
import { createReactor } from "../reactor"
import { createSignal } from "../signal"

const count = () => createSignal(0)

const doubled = () =>
  createComputed((sig) => {
    const value = sig.get(count)
    return value * 2
  })

const quadrupled = () =>
  createComputed((sig) => {
    const value = sig.get(doubled)
    return value * 2
  })

const log = () =>
  createActor((sig) => {
    console.info(sig.get(quadrupled))
  })

const increment = () =>
  createActor((sig) => {
    sig.interval(() => {
      sig.use(count).update((x) => x + 1)
    }, 1000)
  })

const rootActor = () =>
  createActor((sig) => {
    sig.run(log)
    sig.run(increment)
  })

createReactor(rootActor)
