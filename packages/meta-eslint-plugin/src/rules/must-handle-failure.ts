import type { TSESTree } from "@typescript-eslint/types"
import {
  ESLintUtils,
  ParserServicesWithTypeInformation,
  type TSESLint,
} from "@typescript-eslint/utils"
import { unionTypeParts } from "tsutils"
import { SignatureKind, Type, TypeChecker } from "typescript"

import { MessageIds } from "../message-ids"

const FAILURE_UNIQUE_KEY = "dassie.failure"

// evaluates within the expression if it is a failure
// if it is a failure, check if it is assigned or used as an argument for a function
// if it was assigned without handling, check the entire block of the variable for handling
// otherwise it was handled properly

function isFailureLike(checker: TypeChecker, type: Type): boolean {
  for (const ty of unionTypeParts(checker.getApparentType(type))) {
    if (ty.getProperty(FAILURE_UNIQUE_KEY) !== undefined) {
      return true
    }
  }
  return false
}

const endTransverse = ["BlockStatement", "Program"]
function getAssignation(
  checker: TypeChecker,
  parserServices: ParserServicesWithTypeInformation,
  node: TSESTree.Node,
): TSESTree.Identifier | undefined {
  if (
    node.type === "VariableDeclarator" &&
    node.init &&
    isFailureLike(checker, parserServices.getTypeAtLocation(node.init)) &&
    node.id.type === "Identifier"
  ) {
    return node.id
  }
  if (endTransverse.includes(node.type) || !node.parent) {
    return undefined
  }
  return getAssignation(checker, parserServices, node.parent)
}

function isReturnedOrPassed(
  checker: TypeChecker,
  parserServices: ParserServicesWithTypeInformation,
  node: TSESTree.Node,
): boolean {
  if (node.type === "ArrowFunctionExpression") {
    return true
  }
  if (node.type === "ReturnStatement") {
    return true
  }
  if (node.type === "BlockStatement") {
    return false
  }
  if (node.type === "Program") {
    return false
  }
  if (!node.parent) {
    return false
  }
  if (
    node.parent.type === "CallExpression" &&
    (node.parent.arguments as TSESTree.Node[]).includes(node)
  ) {
    const type = parserServices.getTypeAtLocation(node.parent.callee)
    // const signature = checker.getResolvedSignature(node.parent)
    // console.log(signature)

    const signatures = checker.getSignaturesOfType(type, SignatureKind.Call)
    for (const signature of signatures) {
      const typePredicate = checker.getTypePredicateOfSignature(signature)
      if (!typePredicate?.type) continue

      for (const ty of unionTypeParts(
        checker.getApparentType(typePredicate.type),
      )) {
        if (ty.getProperty(FAILURE_UNIQUE_KEY) !== undefined) {
          return true
        }
      }
    }
  }
  return isReturnedOrPassed(checker, parserServices, node.parent)
}

const ignoreParents = [
  "ClassDeclaration",
  "FunctionDeclaration",
  "MethodDefinition",
  "ClassProperty",
  "MemberExpression",
]

function processSelector(
  context: TSESLint.RuleContext<MessageIds, []>,
  checker: TypeChecker,
  parserServices: ParserServicesWithTypeInformation,
  node: TSESTree.Node,
  reportAs = node,
): boolean {
  if (node.parent?.type.startsWith("TS")) {
    return false
  }
  if (node.parent && ignoreParents.includes(node.parent.type)) {
    return false
  }
  if (
    node.parent?.type === "UnaryExpression" &&
    node.parent.operator === "void"
  ) {
    return false
  }
  if (!isFailureLike(checker, parserServices.getTypeAtLocation(node))) {
    return false
  }

  if (isReturnedOrPassed(checker, parserServices, node)) {
    return false
  }

  const assignedTo = getAssignation(checker, parserServices, node)
  const currentScope = context.getScope()

  // Check if is assigned
  if (assignedTo) {
    const variable = currentScope.set.get(assignedTo.name)
    const references =
      variable?.references.filter((ref) => ref.identifier !== assignedTo) ?? []
    if (references.length > 0) {
      return references.some((ref) =>
        processSelector(
          context,
          checker,
          parserServices,
          ref.identifier,
          reportAs,
        ),
      )
    }
  }

  context.report({
    node: reportAs,
    messageId: "mustHandleFailure",
  })
  return true
}

const rule: TSESLint.RuleModule<MessageIds, []> = {
  meta: {
    docs: {
      description:
        "Not handling Failure return values is a possible error because failures could remain unhandled.",
      recommended: "recommended",
      url: "",
    },
    messages: {
      mustHandleFailure:
        "Failures must be returned, handled with isFailure, or be explicitly marked as ignored with the `void` operator.",
    },
    schema: [],
    type: "problem",
  },

  defaultOptions: [],

  create(context) {
    const parserServices = ESLintUtils.getParserServices(context)
    const checker = parserServices.program.getTypeChecker()

    return {
      CallExpression(node: TSESTree.Node) {
        return processSelector(context, checker, parserServices, node)
      },
    }
  },
}

export default rule
