import type { TSESTree } from "@typescript-eslint/types"
import {
  AST_NODE_TYPES,
  ESLintUtils,
  type TSESLint,
} from "@typescript-eslint/utils"
import { isTypeFlagSet, unionTypeParts } from "tsutils"
import {
  CallExpression,
  NewExpression,
  Type,
  TypeChecker,
  TypeFlags,
  TypeReference,
  isCallExpression,
  isComputedPropertyName,
  isMethodDeclaration,
  isObjectLiteralExpression,
  isParameter,
  isPropertyAssignment,
  isShorthandPropertyAssignment,
} from "typescript"

import { createRule } from "../utils/create-rule"
import {
  isAlwaysFailureLike,
  isSometimesFailureLike,
  returnsFailureLike,
} from "../utils/is-failure-like"

type Options = [
  {
    checksConditionals?: boolean
    checksVoidReturn?: ChecksVoidReturnOptions | boolean
    checksSpreads?: boolean
  },
]

interface ChecksVoidReturnOptions {
  arguments?: boolean
  attributes?: boolean
  properties?: boolean
  returns?: boolean
  variables?: boolean
}

type MessageId =
  | "conditional"
  | "spread"
  | "voidReturnArgument"
  | "voidReturnAttribute"
  | "voidReturnProperty"
  | "voidReturnReturnValue"
  | "voidReturnVariable"

function parseChecksVoidReturn(
  checksVoidReturn: ChecksVoidReturnOptions | boolean | undefined,
): ChecksVoidReturnOptions | false {
  switch (checksVoidReturn) {
    case false:
      return false

    case true:
    case undefined:
      return {
        arguments: true,
        attributes: true,
        properties: true,
        returns: true,
        variables: true,
      }

    default:
      return {
        arguments: checksVoidReturn.arguments ?? true,
        attributes: checksVoidReturn.attributes ?? true,
        properties: checksVoidReturn.properties ?? true,
        returns: checksVoidReturn.returns ?? true,
        variables: checksVoidReturn.variables ?? true,
      }
  }
}

export const rule = createRule<Options, MessageId>({
  name: "no-misused-failures",
  meta: {
    docs: {
      description:
        "Disallow Failure values in places not designed to handle them",
      recommended: "recommended",
      requiresTypeChecking: true,
    },
    messages: {
      voidReturnArgument:
        "Failure returned in function argument where a void return was expected.",
      voidReturnVariable:
        "Failure-returning function provided to variable where a void return was expected.",
      voidReturnProperty:
        "Failure-returning function provided to property where a void return was expected.",
      voidReturnReturnValue:
        "Failure-returning function provided to return value where a void return was expected.",
      voidReturnAttribute:
        "Failure-returning function provided to attribute where a void return was expected.",
      conditional: "Expected non-Failure value in a boolean conditional.",
      spread: "Expected a non-Failure value to be spreaded in an object.",
    },
    schema: [
      {
        type: "object",
        additionalProperties: false,
        properties: {
          checksConditionals: {
            type: "boolean",
          },
          checksVoidReturn: {
            oneOf: [
              { type: "boolean" },
              {
                additionalProperties: false,
                properties: {
                  arguments: { type: "boolean" },
                  attributes: { type: "boolean" },
                  properties: { type: "boolean" },
                  returns: { type: "boolean" },
                  variables: { type: "boolean" },
                },
                type: "object",
              },
            ],
          },
          checksSpreads: {
            type: "boolean",
          },
        },
      },
    ],
    type: "problem",
  },

  defaultOptions: [
    {
      checksConditionals: true,
      checksVoidReturn: true,
      checksSpreads: true,
    },
  ],

  create(context, [{ checksConditionals, checksVoidReturn, checksSpreads }]) {
    const services = ESLintUtils.getParserServices(context)
    const checker = services.program.getTypeChecker()

    const checkedNodes = new Set<TSESTree.Node>()

    const conditionalChecks: TSESLint.RuleListener = {
      ConditionalExpression: checkTestConditional,
      DoWhileStatement: checkTestConditional,
      ForStatement: checkTestConditional,
      IfStatement: checkTestConditional,
      LogicalExpression: checkConditional,
      'UnaryExpression[operator="!"]'(node: TSESTree.UnaryExpression) {
        checkConditional(node.argument, true)
      },
      WhileStatement: checkTestConditional,
    }

    checksVoidReturn = parseChecksVoidReturn(checksVoidReturn)

    const voidReturnChecks: TSESLint.RuleListener =
      checksVoidReturn ?
        {
          ...(checksVoidReturn.arguments && {
            CallExpression: checkArguments,
            NewExpression: checkArguments,
          }),
          ...(checksVoidReturn.attributes && {
            JSXAttribute: checkJSXAttribute,
          }),
          ...(checksVoidReturn.properties && {
            Property: checkProperty,
          }),
          ...(checksVoidReturn.returns && {
            ReturnStatement: checkReturnStatement,
          }),
          ...(checksVoidReturn.variables && {
            AssignmentExpression: checkAssignment,
            VariableDeclarator: checkVariableDeclaration,
          }),
        }
      : {}

    const spreadChecks: TSESLint.RuleListener = {
      SpreadElement: checkSpread,
    }

    return {
      ...(checksConditionals ? conditionalChecks : {}),
      ...(checksVoidReturn ? voidReturnChecks : {}),
      ...(checksSpreads ? spreadChecks : {}),
    }

    function checkTestConditional(node: {
      test: TSESTree.Expression | null
    }): void {
      if (node.test) {
        checkConditional(node.test, true)
      }
    }

    /**
     * This function analyzes the type of a node and checks if it is a Failure in a boolean conditional.
     * It uses recursion when checking nested logical operators.
     * @param node The AST node to check.
     * @param isTestExpr Whether the node is a descendant of a test expression.
     */
    function checkConditional(
      node: TSESTree.Expression,
      isTestExpr = false,
    ): void {
      // prevent checking the same node multiple times
      if (checkedNodes.has(node)) {
        return
      }
      checkedNodes.add(node)

      if (node.type === AST_NODE_TYPES.LogicalExpression) {
        // ignore the left operand for nullish coalescing expressions not in a context of a test expression
        if (node.operator !== "??" || isTestExpr) {
          checkConditional(node.left, isTestExpr)
        }
        // we ignore the right operand when not in a context of a test expression
        if (isTestExpr) {
          checkConditional(node.right, isTestExpr)
        }
        return
      }
      if (isAlwaysFailureLike(checker, services.getTypeAtLocation(node))) {
        context.report({
          messageId: "conditional",
          node,
        })
      }
    }

    function checkArguments(
      node: TSESTree.CallExpression | TSESTree.NewExpression,
    ): void {
      const tsNode = services.esTreeNodeToTSNodeMap.get(node)
      const voidArgs = voidFunctionArguments(checker, tsNode)
      if (voidArgs.size === 0) {
        return
      }

      for (const [index, argument] of node.arguments.entries()) {
        if (!voidArgs.has(index)) {
          continue
        }

        const tsNode = services.esTreeNodeToTSNodeMap.get(argument)
        if (returnsFailureLike(checker, checker.getTypeAtLocation(tsNode))) {
          context.report({
            messageId: "voidReturnArgument",
            node: argument,
          })
        }
      }
    }

    function checkAssignment(node: TSESTree.AssignmentExpression): void {
      const tsNode = services.esTreeNodeToTSNodeMap.get(node)
      const varType = services.getTypeAtLocation(node.left)
      if (!isVoidReturningFunctionType(checker, varType)) {
        return
      }

      if (
        returnsFailureLike(checker, checker.getTypeAtLocation(tsNode.right))
      ) {
        context.report({
          messageId: "voidReturnVariable",
          node: node.right,
        })
      }
    }

    function checkVariableDeclaration(node: TSESTree.VariableDeclarator): void {
      const tsNode = services.esTreeNodeToTSNodeMap.get(node)
      if (tsNode.initializer === undefined || node.init == null) {
        return
      }
      const varType = services.getTypeAtLocation(node.id)
      if (!isVoidReturningFunctionType(checker, varType)) {
        return
      }

      if (
        returnsFailureLike(
          checker,
          checker.getTypeAtLocation(tsNode.initializer),
        )
      ) {
        context.report({
          messageId: "voidReturnVariable",
          node: node.init,
        })
      }
    }

    function checkProperty(node: TSESTree.Property): void {
      const tsNode = services.esTreeNodeToTSNodeMap.get(node)
      if (isPropertyAssignment(tsNode)) {
        const contextualType = checker.getContextualType(tsNode.initializer)
        if (
          contextualType !== undefined &&
          isVoidReturningFunctionType(checker, contextualType) &&
          returnsFailureLike(
            checker,
            checker.getTypeAtLocation(tsNode.initializer),
          )
        ) {
          context.report({
            messageId: "voidReturnProperty",
            node: node.value,
          })
        }
      } else if (isShorthandPropertyAssignment(tsNode)) {
        const contextualType = checker.getContextualType(tsNode.name)
        if (
          contextualType !== undefined &&
          isVoidReturningFunctionType(checker, contextualType) &&
          returnsFailureLike(checker, checker.getTypeAtLocation(tsNode.name))
        ) {
          context.report({
            messageId: "voidReturnProperty",
            node: node.value,
          })
        }
      } else if (isMethodDeclaration(tsNode)) {
        if (isComputedPropertyName(tsNode.name)) {
          return
        }
        const obj = tsNode.parent

        // Below condition isn't satisfied unless something goes wrong,
        // but is needed for type checking.
        // 'node' does not include class method declaration so 'obj' is
        // always an object literal expression, but after converting 'node'
        // to TypeScript AST, its type includes MethodDeclaration which
        // does include the case of class method declaration.
        if (!isObjectLiteralExpression(obj)) {
          return
        }

        if (!returnsFailureLike(checker, checker.getTypeAtLocation(tsNode))) {
          return
        }
        const objType = checker.getContextualType(obj)
        if (objType === undefined) {
          return
        }
        const propertySymbol = checker.getPropertyOfType(
          objType,
          tsNode.name.text,
        )
        if (propertySymbol === undefined) {
          return
        }

        const contextualType = checker.getTypeOfSymbolAtLocation(
          propertySymbol,
          tsNode.name,
        )

        if (isVoidReturningFunctionType(checker, contextualType)) {
          context.report({
            messageId: "voidReturnProperty",
            node: node.value,
          })
        }
        return
      }
    }

    function checkReturnStatement(node: TSESTree.ReturnStatement): void {
      const tsNode = services.esTreeNodeToTSNodeMap.get(node)
      if (tsNode.expression === undefined || node.argument == null) {
        return
      }
      const contextualType = checker.getContextualType(tsNode.expression)
      if (
        contextualType !== undefined &&
        isVoidReturningFunctionType(checker, contextualType) &&
        returnsFailureLike(
          checker,
          checker.getTypeAtLocation(tsNode.expression),
        )
      ) {
        context.report({
          messageId: "voidReturnReturnValue",
          node: node.argument,
        })
      }
    }

    function checkJSXAttribute(node: TSESTree.JSXAttribute): void {
      if (
        node.value == null ||
        node.value.type !== AST_NODE_TYPES.JSXExpressionContainer
      ) {
        return
      }
      const expressionContainer = services.esTreeNodeToTSNodeMap.get(node.value)
      const expression = services.esTreeNodeToTSNodeMap.get(
        node.value.expression,
      )
      const contextualType = checker.getContextualType(expressionContainer)
      if (
        contextualType !== undefined &&
        isVoidReturningFunctionType(checker, contextualType) &&
        returnsFailureLike(checker, checker.getTypeAtLocation(expression))
      ) {
        context.report({
          messageId: "voidReturnAttribute",
          node: node.value,
        })
      }
    }

    function checkSpread(node: TSESTree.SpreadElement): void {
      const tsNode = services.esTreeNodeToTSNodeMap.get(node)

      if (
        isSometimesFailureLike(
          checker,
          checker.getTypeAtLocation(tsNode.expression),
        )
      ) {
        context.report({
          messageId: "spread",
          node: node.argument,
        })
      }
    }
  },
})

function checkThenableOrVoidArgument(
  checker: TypeChecker,
  type: Type,
  index: number,
  thenableReturnIndices: Set<number>,
  voidReturnIndices: Set<number>,
): void {
  if (isFailureLikeReturningFunctionType(checker, type)) {
    thenableReturnIndices.add(index)
  } else if (isVoidReturningFunctionType(checker, type)) {
    // If a certain argument accepts both thenable and void returns,
    // a promise-returning function is valid
    if (!thenableReturnIndices.has(index)) {
      voidReturnIndices.add(index)
    }
  }
}

// Get the positions of arguments which are void functions (and not also
// failure-returning functions). These are the candidates for the void-return
// check at the current call site.
// If the function parameters end with a 'rest' parameter, then we consider
// the array type parameter (e.g. '...args:Array<SomeType>') when determining
// if trailing arguments are candidates.
function voidFunctionArguments(
  checker: TypeChecker,
  node: CallExpression | NewExpression,
): Set<number> {
  // 'new' can be used without any arguments, as in 'let b = new Object;'
  // In this case, there are no argument positions to check, so return early.
  if (!node.arguments) {
    return new Set<number>()
  }
  const thenableReturnIndices = new Set<number>()
  const voidReturnIndices = new Set<number>()
  const type = checker.getTypeAtLocation(node.expression)

  // We can't use checker.getResolvedSignature because it prefers an early '() => void' over a later '() => Failure'
  // See https://github.com/microsoft/TypeScript/issues/48077

  for (const subType of unionTypeParts(type)) {
    // Standard function calls and `new` have two different types of signatures
    const signatures =
      isCallExpression(node) ?
        subType.getCallSignatures()
      : subType.getConstructSignatures()
    for (const signature of signatures) {
      for (const [index, parameter] of signature.parameters.entries()) {
        const decl = parameter.valueDeclaration
        let type = checker.getTypeOfSymbolAtLocation(parameter, node.expression)

        // If this is a array 'rest' parameter, check all of the argument indices
        // from the current argument to the end.
        // Note - we currently do not support 'spread' arguments - adding support for them
        // is tracked in https://github.com/typescript-eslint/typescript-eslint/issues/5744
        if (decl && isParameter(decl) && decl.dotDotDotToken) {
          if (checker.isArrayType(type)) {
            // Unwrap 'Array<MaybeVoidFunction>' to 'MaybeVoidFunction',
            // so that we'll handle it in the same way as a non-rest
            // 'param: MaybeVoidFunction'
            type = checker.getTypeArguments(type as TypeReference)[0]!
            for (let i = index; i < node.arguments.length; i++) {
              checkThenableOrVoidArgument(
                checker,
                type,
                i,
                thenableReturnIndices,
                voidReturnIndices,
              )
            }
          } else if (checker.isTupleType(type)) {
            // Check each type in the tuple - for example, [boolean, () => void] would
            // add the index of the second tuple parameter to 'voidReturnIndices'
            const typeArgs = checker.getTypeArguments(type as TypeReference)
            for (
              let i = index;
              i < node.arguments.length && i - index < typeArgs.length;
              i++
            ) {
              checkThenableOrVoidArgument(
                checker,
                typeArgs[i - index]!,
                i,
                thenableReturnIndices,
                voidReturnIndices,
              )
            }
          }
        } else {
          checkThenableOrVoidArgument(
            checker,
            type,
            index,
            thenableReturnIndices,
            voidReturnIndices,
          )
        }
      }
    }
  }

  for (const index of thenableReturnIndices) {
    voidReturnIndices.delete(index)
  }

  return voidReturnIndices
}

/**
 * @returns Whether type is a thenable-returning function.
 */
function isFailureLikeReturningFunctionType(
  checker: TypeChecker,
  type: Type,
): boolean {
  for (const subType of unionTypeParts(type)) {
    if (returnsFailureLike(checker, subType)) {
      return true
    }
  }

  return false
}

/**
 * @returns Whether type is a void-returning function.
 */
function isVoidReturningFunctionType(
  checker: TypeChecker,
  type: Type,
): boolean {
  let hadVoidReturn = false

  for (const subType of unionTypeParts(type)) {
    for (const signature of subType.getCallSignatures()) {
      const returnType = signature.getReturnType()

      if (isSometimesFailureLike(checker, returnType)) {
        return false
      }

      hadVoidReturn ||= isTypeFlagSet(returnType, TypeFlags.Void)
    }
  }

  return hadVoidReturn
}
