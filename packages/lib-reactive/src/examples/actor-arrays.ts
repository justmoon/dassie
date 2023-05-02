import { createActor } from "../actor"
import { createReactor } from "../reactor"
import { createSignal } from "../signal"

const listSignal = () => createSignal<readonly string[]>([])

const itemActor = () =>
  createActor((sig, entry: string) => {
    console.log("List entry added:", entry)

    sig.onCleanup(() => {
      console.log("List entry removed:", entry)
    })
  })

const rootEffect = () =>
  createActor((sig) => {
    sig.for(listSignal, itemActor)

    sig.timeout(() => {
      console.log("Adding 'Hello'")
      sig.use(listSignal).update((list) => [...list, "Hello"])
    }, 100)

    sig.timeout(() => {
      console.log("Adding 'world!'")
      sig.use(listSignal).update((list) => [...list, "world!"])
    }, 200)

    sig.timeout(() => {
      console.log("Adding 'reactive'")
      sig.use(listSignal).update((list) => [list[0]!, "reactive", list[1]!])
    }, 300)

    sig.timeout(() => {
      console.log("Replacing 'reactive' and 'world!' with 'and' and 'goodbye!'")
      sig.use(listSignal).update((list) => [list[0]!, "and", "goodbye!"])
    }, 400)

    sig.timeout(() => {
      console.log("Disposing reactor")
      void sig.reactor.dispose()
    }, 500)
  })

createReactor(rootEffect)
