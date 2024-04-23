import { setTimeout } from "node:timers/promises"

import { header } from "../components/header"
import { note } from "../components/note"
import { progress } from "../components/progress"
import { createFlow } from "../flow"

const flow = createFlow()

flow.show(header({ title: "Example: Progress" }))

flow.show(note({ title: `Exploring the unknown...` }))

for (let index = 0; index < 3; index++) {
  await flow.attach(
    progress({ description: `Sector ${index + 1}` }),
    async () => {
      await setTimeout(1000)
    },
  )
}

flow.show(note({ title: `Manufacturing toasters...` }))

for (let index = 0; index < 3; index++) {
  await flow.attach(
    progress({ description: `Toaster ${index + 1}` }),
    async (state) => {
      for (let progress = 0; progress <= 1; progress += 1 / 150) {
        state.setProgress(progress)
        await setTimeout(20)
      }
    },
  )
}
