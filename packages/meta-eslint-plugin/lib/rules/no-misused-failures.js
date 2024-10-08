"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.rule = void 0;
const utils_1 = require("@typescript-eslint/utils");
const tsutils_1 = require("tsutils");
const typescript_1 = require("typescript");
const create_rule_1 = require("../utils/create-rule");
const is_failure_like_1 = require("../utils/is-failure-like");
function parseChecksVoidReturn(checksVoidReturn) {
    switch (checksVoidReturn) {
        case false:
            return false;
        case true:
        case undefined:
            return {
                arguments: true,
                attributes: true,
                properties: true,
                returns: true,
                variables: true,
            };
        default:
            return {
                arguments: checksVoidReturn.arguments ?? true,
                attributes: checksVoidReturn.attributes ?? true,
                properties: checksVoidReturn.properties ?? true,
                returns: checksVoidReturn.returns ?? true,
                variables: checksVoidReturn.variables ?? true,
            };
    }
}
exports.rule = (0, create_rule_1.createRule)({
    name: "no-misused-failures",
    meta: {
        docs: {
            description: "Disallow Failure values in places not designed to handle them",
        },
        messages: {
            voidReturnArgument: "Failure returned in function argument where a void return was expected.",
            voidReturnVariable: "Failure-returning function provided to variable where a void return was expected.",
            voidReturnProperty: "Failure-returning function provided to property where a void return was expected.",
            voidReturnReturnValue: "Failure-returning function provided to return value where a void return was expected.",
            voidReturnAttribute: "Failure-returning function provided to attribute where a void return was expected.",
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
        const services = utils_1.ESLintUtils.getParserServices(context);
        const checker = services.program.getTypeChecker();
        const checkedNodes = new Set();
        const conditionalChecks = {
            ConditionalExpression: checkTestConditional,
            DoWhileStatement: checkTestConditional,
            ForStatement: checkTestConditional,
            IfStatement: checkTestConditional,
            LogicalExpression: checkConditional,
            'UnaryExpression[operator="!"]'(node) {
                checkConditional(node.argument, true);
            },
            WhileStatement: checkTestConditional,
        };
        checksVoidReturn = parseChecksVoidReturn(checksVoidReturn);
        const voidReturnChecks = checksVoidReturn ?
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
            : {};
        const spreadChecks = {
            SpreadElement: checkSpread,
        };
        return {
            ...(checksConditionals ? conditionalChecks : {}),
            ...(checksVoidReturn ? voidReturnChecks : {}),
            ...(checksSpreads ? spreadChecks : {}),
        };
        function checkTestConditional(node) {
            if (node.test) {
                checkConditional(node.test, true);
            }
        }
        /**
         * This function analyzes the type of a node and checks if it is a Failure in a boolean conditional.
         * It uses recursion when checking nested logical operators.
         * @param node The AST node to check.
         * @param isTestExpr Whether the node is a descendant of a test expression.
         */
        function checkConditional(node, isTestExpr = false) {
            // prevent checking the same node multiple times
            if (checkedNodes.has(node)) {
                return;
            }
            checkedNodes.add(node);
            if (node.type === utils_1.AST_NODE_TYPES.LogicalExpression) {
                // ignore the left operand for nullish coalescing expressions not in a context of a test expression
                if (node.operator !== "??" || isTestExpr) {
                    checkConditional(node.left, isTestExpr);
                }
                // we ignore the right operand when not in a context of a test expression
                if (isTestExpr) {
                    checkConditional(node.right, isTestExpr);
                }
                return;
            }
            if ((0, is_failure_like_1.isAlwaysFailureLike)(checker, services.getTypeAtLocation(node))) {
                context.report({
                    messageId: "conditional",
                    node,
                });
            }
        }
        function checkArguments(node) {
            const tsNode = services.esTreeNodeToTSNodeMap.get(node);
            const voidArgs = voidFunctionArguments(checker, tsNode);
            if (voidArgs.size === 0) {
                return;
            }
            for (const [index, argument] of node.arguments.entries()) {
                if (!voidArgs.has(index)) {
                    continue;
                }
                const tsNode = services.esTreeNodeToTSNodeMap.get(argument);
                if ((0, is_failure_like_1.returnsFailureLike)(checker, checker.getTypeAtLocation(tsNode))) {
                    context.report({
                        messageId: "voidReturnArgument",
                        node: argument,
                    });
                }
            }
        }
        function checkAssignment(node) {
            const tsNode = services.esTreeNodeToTSNodeMap.get(node);
            const varType = services.getTypeAtLocation(node.left);
            if (!isVoidReturningFunctionType(checker, varType)) {
                return;
            }
            if ((0, is_failure_like_1.returnsFailureLike)(checker, checker.getTypeAtLocation(tsNode.right))) {
                context.report({
                    messageId: "voidReturnVariable",
                    node: node.right,
                });
            }
        }
        function checkVariableDeclaration(node) {
            const tsNode = services.esTreeNodeToTSNodeMap.get(node);
            if (tsNode.initializer === undefined || node.init == null) {
                return;
            }
            const varType = services.getTypeAtLocation(node.id);
            if (!isVoidReturningFunctionType(checker, varType)) {
                return;
            }
            if ((0, is_failure_like_1.returnsFailureLike)(checker, checker.getTypeAtLocation(tsNode.initializer))) {
                context.report({
                    messageId: "voidReturnVariable",
                    node: node.init,
                });
            }
        }
        function checkProperty(node) {
            const tsNode = services.esTreeNodeToTSNodeMap.get(node);
            if ((0, typescript_1.isPropertyAssignment)(tsNode)) {
                const contextualType = checker.getContextualType(tsNode.initializer);
                if (contextualType !== undefined &&
                    isVoidReturningFunctionType(checker, contextualType) &&
                    (0, is_failure_like_1.returnsFailureLike)(checker, checker.getTypeAtLocation(tsNode.initializer))) {
                    context.report({
                        messageId: "voidReturnProperty",
                        node: node.value,
                    });
                }
            }
            else if ((0, typescript_1.isShorthandPropertyAssignment)(tsNode)) {
                const contextualType = checker.getContextualType(tsNode.name);
                if (contextualType !== undefined &&
                    isVoidReturningFunctionType(checker, contextualType) &&
                    (0, is_failure_like_1.returnsFailureLike)(checker, checker.getTypeAtLocation(tsNode.name))) {
                    context.report({
                        messageId: "voidReturnProperty",
                        node: node.value,
                    });
                }
            }
            else if ((0, typescript_1.isMethodDeclaration)(tsNode)) {
                if ((0, typescript_1.isComputedPropertyName)(tsNode.name)) {
                    return;
                }
                const obj = tsNode.parent;
                // Below condition isn't satisfied unless something goes wrong,
                // but is needed for type checking.
                // 'node' does not include class method declaration so 'obj' is
                // always an object literal expression, but after converting 'node'
                // to TypeScript AST, its type includes MethodDeclaration which
                // does include the case of class method declaration.
                if (!(0, typescript_1.isObjectLiteralExpression)(obj)) {
                    return;
                }
                if (!(0, is_failure_like_1.returnsFailureLike)(checker, checker.getTypeAtLocation(tsNode))) {
                    return;
                }
                const objType = checker.getContextualType(obj);
                if (objType === undefined) {
                    return;
                }
                const propertySymbol = checker.getPropertyOfType(objType, tsNode.name.text);
                if (propertySymbol === undefined) {
                    return;
                }
                const contextualType = checker.getTypeOfSymbolAtLocation(propertySymbol, tsNode.name);
                if (isVoidReturningFunctionType(checker, contextualType)) {
                    context.report({
                        messageId: "voidReturnProperty",
                        node: node.value,
                    });
                }
                return;
            }
        }
        function checkReturnStatement(node) {
            const tsNode = services.esTreeNodeToTSNodeMap.get(node);
            if (tsNode.expression === undefined || node.argument == null) {
                return;
            }
            const contextualType = checker.getContextualType(tsNode.expression);
            if (contextualType !== undefined &&
                isVoidReturningFunctionType(checker, contextualType) &&
                (0, is_failure_like_1.returnsFailureLike)(checker, checker.getTypeAtLocation(tsNode.expression))) {
                context.report({
                    messageId: "voidReturnReturnValue",
                    node: node.argument,
                });
            }
        }
        function checkJSXAttribute(node) {
            if (node.value == null ||
                node.value.type !== utils_1.AST_NODE_TYPES.JSXExpressionContainer) {
                return;
            }
            const expressionContainer = services.esTreeNodeToTSNodeMap.get(node.value);
            const expression = services.esTreeNodeToTSNodeMap.get(node.value.expression);
            const contextualType = checker.getContextualType(expressionContainer);
            if (contextualType !== undefined &&
                isVoidReturningFunctionType(checker, contextualType) &&
                (0, is_failure_like_1.returnsFailureLike)(checker, checker.getTypeAtLocation(expression))) {
                context.report({
                    messageId: "voidReturnAttribute",
                    node: node.value,
                });
            }
        }
        function checkSpread(node) {
            const tsNode = services.esTreeNodeToTSNodeMap.get(node);
            if ((0, is_failure_like_1.isSometimesFailureLike)(checker, checker.getTypeAtLocation(tsNode.expression))) {
                context.report({
                    messageId: "spread",
                    node: node.argument,
                });
            }
        }
    },
});
function checkThenableOrVoidArgument(checker, type, index, thenableReturnIndices, voidReturnIndices) {
    if (isFailureLikeReturningFunctionType(checker, type)) {
        thenableReturnIndices.add(index);
    }
    else if (isVoidReturningFunctionType(checker, type)) {
        // If a certain argument accepts both thenable and void returns,
        // a promise-returning function is valid
        if (!thenableReturnIndices.has(index)) {
            voidReturnIndices.add(index);
        }
    }
}
// Get the positions of arguments which are void functions (and not also
// failure-returning functions). These are the candidates for the void-return
// check at the current call site.
// If the function parameters end with a 'rest' parameter, then we consider
// the array type parameter (e.g. '...args:Array<SomeType>') when determining
// if trailing arguments are candidates.
function voidFunctionArguments(checker, node) {
    // 'new' can be used without any arguments, as in 'let b = new Object;'
    // In this case, there are no argument positions to check, so return early.
    if (!node.arguments) {
        return new Set();
    }
    const thenableReturnIndices = new Set();
    const voidReturnIndices = new Set();
    const type = checker.getTypeAtLocation(node.expression);
    // We can't use checker.getResolvedSignature because it prefers an early '() => void' over a later '() => Failure'
    // See https://github.com/microsoft/TypeScript/issues/48077
    for (const subType of (0, tsutils_1.unionTypeParts)(type)) {
        // Standard function calls and `new` have two different types of signatures
        const signatures = (0, typescript_1.isCallExpression)(node) ?
            subType.getCallSignatures()
            : subType.getConstructSignatures();
        for (const signature of signatures) {
            for (const [index, parameter] of signature.parameters.entries()) {
                const decl = parameter.valueDeclaration;
                let type = checker.getTypeOfSymbolAtLocation(parameter, node.expression);
                // If this is a array 'rest' parameter, check all of the argument indices
                // from the current argument to the end.
                // Note - we currently do not support 'spread' arguments - adding support for them
                // is tracked in https://github.com/typescript-eslint/typescript-eslint/issues/5744
                if (decl && (0, typescript_1.isParameter)(decl) && decl.dotDotDotToken) {
                    if (checker.isArrayType(type)) {
                        // Unwrap 'Array<MaybeVoidFunction>' to 'MaybeVoidFunction',
                        // so that we'll handle it in the same way as a non-rest
                        // 'param: MaybeVoidFunction'
                        type = checker.getTypeArguments(type)[0];
                        for (let i = index; i < node.arguments.length; i++) {
                            checkThenableOrVoidArgument(checker, type, i, thenableReturnIndices, voidReturnIndices);
                        }
                    }
                    else if (checker.isTupleType(type)) {
                        // Check each type in the tuple - for example, [boolean, () => void] would
                        // add the index of the second tuple parameter to 'voidReturnIndices'
                        const typeArgs = checker.getTypeArguments(type);
                        for (let i = index; i < node.arguments.length && i - index < typeArgs.length; i++) {
                            checkThenableOrVoidArgument(checker, typeArgs[i - index], i, thenableReturnIndices, voidReturnIndices);
                        }
                    }
                }
                else {
                    checkThenableOrVoidArgument(checker, type, index, thenableReturnIndices, voidReturnIndices);
                }
            }
        }
    }
    for (const index of thenableReturnIndices) {
        voidReturnIndices.delete(index);
    }
    return voidReturnIndices;
}
/**
 * @returns Whether type is a thenable-returning function.
 */
function isFailureLikeReturningFunctionType(checker, type) {
    for (const subType of (0, tsutils_1.unionTypeParts)(type)) {
        if ((0, is_failure_like_1.returnsFailureLike)(checker, subType)) {
            return true;
        }
    }
    return false;
}
/**
 * @returns Whether type is a void-returning function.
 */
function isVoidReturningFunctionType(checker, type) {
    let hadVoidReturn = false;
    for (const subType of (0, tsutils_1.unionTypeParts)(type)) {
        for (const signature of subType.getCallSignatures()) {
            const returnType = signature.getReturnType();
            if ((0, is_failure_like_1.isSometimesFailureLike)(checker, returnType)) {
                return false;
            }
            hadVoidReturn ||= (0, tsutils_1.isTypeFlagSet)(returnType, typescript_1.TypeFlags.Void);
        }
    }
    return hadVoidReturn;
}
