import { Effect, createReactor } from "../create-reactor"
import { createStore } from "../create-store"

const counterStore = createStore("counter", 0)

const rootEffect: Effect = (sig) => {
  sig.interval(() => {
    sig.reactor.emit(counterStore, (a) => a + 1)
  }, 500)

  void sig.use(innerEffect)
}

const innerEffect: Effect = async (sig) => {
  const counter = sig.get(counterStore)

  // This will only print every three seconds or so
  console.log(counter)

  // Wait long enough so the counter will have updated a bunch of times before we're ready to run again
  await new Promise((resolve) => setTimeout(resolve, 3000))
}

createReactor(rootEffect)
