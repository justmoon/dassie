import { enableMapSet, produce } from "immer"

import { setImmediate } from "node:timers/promises"

import { createActor } from "../actor"
import { createMapped } from "../mapped"
import { type Reactor, createReactor } from "../reactor"
import { createSignal } from "../signal"

enableMapSet()

const CustomersSignal = () => createSignal<Set<string>>(new Set())

const CustomerServiceActors = (reactor: Reactor) =>
  createMapped(reactor, CustomersSignal, (customer) =>
    createActor((sig) => {
      console.info(`${customer} added`)

      sig.onCleanup(() => {
        console.info(`${customer} removed`)
      })

      return {
        greet: () => {
          console.info(`Hello ${customer}`)
        },
      }
    }),
  )

const RootActor = (reactor: Reactor) =>
  createActor(async (sig) => {
    sig.runMap(CustomerServiceActors)

    const customers = sig.reactor.use(CustomersSignal)

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

    await setImmediate()

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

    await sig.reactor.use(CustomerServiceActors).get("Alice")?.api.greet.ask()

    void reactor.dispose()
  })

createReactor(RootActor)
