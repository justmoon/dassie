import { AST_NODE_TYPES, type TSESTree } from "@typescript-eslint/types"

export function isTopLevel(node: TSESTree.Node) {
  let scope = node.parent
  while (scope?.type === AST_NODE_TYPES.BlockStatement) {
    scope = scope.parent
  }
  return scope?.type === AST_NODE_TYPES.Program
}
