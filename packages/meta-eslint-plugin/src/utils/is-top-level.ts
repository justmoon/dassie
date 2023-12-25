import type { TSESTree } from "@typescript-eslint/types"

export function isTopLevel(node: TSESTree.Node) {
  let scope = node.parent
  while (scope?.type === "BlockStatement") {
    scope = scope.parent
  }
  return scope?.type === "Program"
}
