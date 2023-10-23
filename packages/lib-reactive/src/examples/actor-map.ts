import { enableMapSet, produce } from "immer"

import { createActor } from "../actor"
import { createMapped } from "../mapped"
import { Reactor, createReactor } from "../reactor"
import { createSignal } from "../signal"

enableMapSet()

const CustomersSignal = () => createSignal<Set<string>>(new Set())

const CustomerServiceActors = (reactor: Reactor) =>
  createMapped(reactor.lifecycle, reactor.use(CustomersSignal), (customer) =>
    createActor((sig) => {
      console.info(`${customer} added`)

      sig.onCleanup(() => console.info(`${customer} removed`))

      return sig.handlers({
        greet: () => console.info(`Hello ${customer}`),
      })
    }),
  )

const RootActor = (reactor: Reactor) =>
  createActor((sig) => {
    sig.runMap(CustomerServiceActors)

    const customers = sig.use(CustomersSignal)

    customers.update(
      produce((draft) => {
        draft.add("Alice")
      }),
    )
    customers.update(
      produce((draft) => {
        draft.add("Bob")
      }),
    )
    customers.update(
      produce((draft) => {
        draft.add("Charlie")
      }),
    )
    customers.update(
      produce((draft) => {
        draft.delete("Bob")
      }),
    )
    customers.update(
      produce((draft) => {
        draft.add("Denise")
      }),
    )

    sig.use(CustomerServiceActors).get("Alice")?.api.greet.tell()

    sig.timeout(() => void reactor.lifecycle.dispose(), 300)
  })

createReactor(RootActor)
