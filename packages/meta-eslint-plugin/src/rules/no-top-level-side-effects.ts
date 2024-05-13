import type { TSESTree } from "@typescript-eslint/types"

import { createRule } from "../utils/create-rule"
import { isTopLevel } from "../utils/is-top-level"

type Options = [
  {
    allowSimpleAssignments?: boolean
    allowInExportlessModules?: boolean
    allowedNodes?: Array<string | { selector: string }>
  },
]
type MessageId = "noTopLevelSideEffect"

export const rule = createRule<Options, MessageId>({
  name: "no-top-level-side-effects",
  meta: {
    docs: {
      description:
        "There should be no side effects outside of functions. Note that this rule only checks for some types of side effects and ignores others, most notably function calls that happen in variable declarations. The reason is that many variable declarations may legitimately contain pure functions used for initialization and it would be too noisy to report on all of them.",
      recommended: "recommended",
    },
    messages: {
      noTopLevelSideEffect: "Your modules should not have any side effects",
    },
    schema: [
      {
        type: "object",
        properties: {
          allowSimpleAssignments: { type: "boolean" },
          allowInExportlessModules: { type: "boolean" },
          allowedNodes: {
            type: "array",
            items: {
              oneOf: [
                {
                  type: "string",
                },
                {
                  type: "object",
                  properties: {
                    selector: { type: "string" },
                  },
                  required: ["selector"],
                  additionalProperties: false,
                },
              ],
            },
            uniqueItems: true,
            minItems: 0,
          },
        },
        additionalProperties: false,
      },
    ],
    type: "problem",
  },

  defaultOptions: [{}],

  create(context, [option]) {
    const allowedNodes =
      option.allowedNodes?.map((option) =>
        typeof option === "string" ? option : option.selector,
      ) ?? []

    if (option.allowSimpleAssignments) {
      allowedNodes.push(
        "ExpressionStatement[expression.type=AssignmentExpression][expression.right.type=/^(Literal|MemberExpression|ArrowFunctionExpression|CallExpression)$/]",
      )
    }

    if (option.allowInExportlessModules) {
      allowedNodes.push("Program:not(:has(ExportNamedDeclaration)) *")
    }

    const constructSelector = (nodeType: string) =>
      allowedNodes.length ?
        `${nodeType}:not(:matches(${allowedNodes.join(",")}))`
      : nodeType

    return {
      [constructSelector("ExpressionStatement")]: checkForTopLevelSideEffect,
      [constructSelector("IfStatement")]: checkForTopLevelSideEffect,
      [constructSelector("SwitchStatement")]: checkForTopLevelSideEffect,
      [constructSelector("TryStatement")]: checkForTopLevelSideEffect,
      [constructSelector("WhileStatement")]: checkForTopLevelSideEffect,
      [constructSelector("ForStatement")]: checkForTopLevelSideEffect,
      [constructSelector("ThrowStatement")]: checkForTopLevelSideEffect,
    }

    function checkForTopLevelSideEffect(node: TSESTree.Node) {
      if (isTopLevel(node)) {
        context.report({
          node,
          messageId: "noTopLevelSideEffect",
        })
      }
    }
  },
})
