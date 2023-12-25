import { createRule } from "../utils/create-rule"
import { isTopLevel } from "../utils/is-top-level"

type Options = []
type MessageId = "noTopLevelMutable"

export const rule = createRule<Options, MessageId>({
  name: "no-top-level-mutables",
  meta: {
    docs: {
      description: "There should be no mutable state outside of functions.",
      recommended: "recommended",
    },
    messages: {
      noTopLevelMutable:
        "Using {{clause}} at the top level creates global, mutable state. Wrap your state in a factory function so that it is scoped to a specific instance of your code.",
    },
    schema: [],
    type: "problem",
  },

  defaultOptions: [],

  create(context) {
    return {
      VariableDeclaration(node) {
        if ((node.kind === "let" || node.kind === "var") && isTopLevel(node)) {
          context.report({
            node,
            messageId: "noTopLevelMutable",
            data: { clause: node.kind },
          })
        }
      },
    }
  },
})
