import { setTimeout } from "node:timers/promises"

import { header } from "../components/header"
import { note } from "../components/note"
import { tasklist } from "../components/tasklist"
import { createFlow } from "../flow"

const flow = createFlow()

flow.show(header({ title: "Example: Tasklist" }))

flow.show(note({ title: `Living life...` }))

await flow.attach(tasklist({}), async (state) => {
  state.addTask("wake-up", { description: "Wake up", progress: "done" })
  await setTimeout(200)
  state.addTask("think-coffee", {
    description: "Think about coffee",
    progress: "indeterminate",
  })
  await setTimeout(1000)
  state.addTask("go-to-work", {
    description: "Go to work",
    progress: "indeterminate",
  })
  await setTimeout(1000)
  state.updateTask("go-to-work", (task) => ({
    ...task,
    progress: "done",
  }))
  state.addTask("drink-coffee", {
    description: "Drink coffee",
    progress: "indeterminate",
  })
  await setTimeout(1000)
  state.updateTask("drink-coffee", (task) => ({
    ...task,
    progress: "done",
  }))
  state.updateTask("think-coffee", (task) => ({
    ...task,
    progress: "done",
  }))
  state.addTask("eat-lunch", {
    description: "Eat lunch",
    progress: "indeterminate",
  })
  await setTimeout(1000)
  state.updateTask("eat-lunch", (task) => ({
    ...task,
    progress: "done",
  }))
  state.addTask("go-home", {
    description: "Go home",
    progress: "indeterminate",
  })
  await setTimeout(1000)
  state.updateTask("go-home", (task) => ({
    ...task,
    progress: "done",
  }))
  state.addTask("eat-dinner", {
    description: "Eat dinner",
    progress: "indeterminate",
  })
  await setTimeout(1000)
  state.updateTask("eat-dinner", (task) => ({
    ...task,
    progress: "done",
  }))
  state.addTask("go-bed", {
    description: "Go to bed",
    progress: "indeterminate",
  })
  await setTimeout(1000)
  state.updateTask("go-bed", (task) => ({
    ...task,
    progress: "done",
  }))
})
