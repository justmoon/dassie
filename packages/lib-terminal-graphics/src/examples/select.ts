import { header } from "../components/header"
import { select } from "../components/select"
import { createFlow } from "../flow"

const flow = createFlow()

flow.show(header({ title: "Example: Select" }))

await flow.interact(
  select({
    title: "Select something...",
    choices: [
      {
        value: "one",
        label: "One",
        description: "This is the first option",
      },
      {
        value: "two",
        label: "Two",
        description:
          "This is the second option and it happens to have a really long description that will wrap to the next line and create a real mess of things. Someone should really fix this. Or maybe the library will handle it perfectly fine?",
      },
    ],
  }),
)

await flow.interact(
  select({
    title: "Select something else...",
    choices: [
      {
        value: "one",
        label: "One",
      },
      {
        value: "two",
        label: "Two",
      },
    ],
  }),
)
