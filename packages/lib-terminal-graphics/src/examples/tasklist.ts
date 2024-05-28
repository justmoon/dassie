import { setTimeout } from "node:timers/promises"

import { header } from "../components/header"
import { note } from "../components/note"
import { tasklist } from "../components/tasklist"
import { createFlow } from "../flow"

const flow = createFlow()

flow.show(header({ title: "Example: Tasklist" }))

flow.show(note({ title: `Living life...` }))

await flow.attach(tasklist({}), async (state) => {
  state.act.addTask("wake-up", { description: "Wake up", progress: "done" })
  await setTimeout(200)
  state.act.addTask("think-coffee", {
    description: "Think about coffee",
    progress: "indeterminate",
  })
  await setTimeout(1000)
  state.act.addTask("go-to-work", {
    description: "Go to work",
    progress: "indeterminate",
  })
  await setTimeout(1000)
  state.act.updateTask("go-to-work", {
    progress: "done",
  })
  state.act.addTask("drink-coffee", {
    description: "Drink coffee",
    progress: "indeterminate",
  })
  await setTimeout(1000)
  state.act.updateTask("drink-coffee", {
    progress: "done",
  })
  state.act.updateTask("think-coffee", {
    progress: "done",
  })
  state.act.addTask("eat-lunch", {
    description: "Eat lunch",
    progress: "indeterminate",
  })
  await setTimeout(1000)
  state.act.updateTask("eat-lunch", {
    progress: "done",
  })
  state.act.addTask("go-home", {
    description: "Go home",
    progress: "indeterminate",
  })
  await setTimeout(1000)
  state.act.updateTask("go-home", {
    progress: "done",
  })
  state.act.addTask("eat-dinner", {
    description: "Eat dinner",
    progress: "indeterminate",
  })
  await setTimeout(1000)
  state.act.updateTask("eat-dinner", {
    progress: "done",
  })
  state.act.addTask("go-bed", {
    description: "Go to bed",
    progress: "indeterminate",
  })
  await setTimeout(1000)
  state.act.updateTask("go-bed", {
    progress: "done",
  })
})
