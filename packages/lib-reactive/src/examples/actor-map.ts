import { enableMapSet, produce } from "immer"

import { createActor } from "../actor"
import { createMapped } from "../mapped"
import { createReactor } from "../reactor"
import { createSignal } from "../signal"

enableMapSet()

const customersSignal = () => createSignal<Set<string>>(new Set())

const customerServiceActors = () =>
  createMapped(customersSignal, (customer) =>
    createActor((sig) => {
      console.info(`${customer} added`)

      sig.onCleanup(() => console.info(`${customer} removed`))

      return {
        greet: () => console.info(`Hello ${customer}`),
      }
    })
  )

const rootActor = () =>
  createActor((sig) => {
    sig.runMap(customerServiceActors)

    const customers = sig.use(customersSignal)

    customers.update(
      produce((draft) => {
        draft.add("Alice")
      })
    )
    customers.update(
      produce((draft) => {
        draft.add("Bob")
      })
    )
    customers.update(
      produce((draft) => {
        draft.add("Charlie")
      })
    )
    customers.update(
      produce((draft) => {
        draft.delete("Bob")
      })
    )
    customers.update(
      produce((draft) => {
        draft.add("Denise")
      })
    )

    sig.use(customerServiceActors).get("Alice")?.tell("greet", undefined)

    sig.timeout(() => void sig.reactor.dispose(), 300)
  })

createReactor(rootActor)
