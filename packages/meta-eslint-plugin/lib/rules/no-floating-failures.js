"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.rule = void 0;
const utils_1 = require("@typescript-eslint/utils");
const create_rule_1 = require("../utils/create-rule");
const is_failure_like_1 = require("../utils/is-failure-like");
exports.rule = (0, create_rule_1.createRule)({
    name: "no-floating-failures",
    meta: {
        docs: {
            description: "Not handling Failure return values is a possible error because failures could remain unhandled.",
        },
        hasSuggestions: true,
        messages: {
            noFloatingFailures: "Failures must be returned, handled with isFailure, or be explicitly marked as ignored with the `void` operator.",
            floatingFixVoid: "Add void operator to ignore.",
        },
        schema: [],
        type: "problem",
    },
    defaultOptions: [],
    create(context) {
        const services = utils_1.ESLintUtils.getParserServices(context);
        const checker = services.program.getTypeChecker();
        return {
            ExpressionStatement(node) {
                let expression = node.expression;
                if (expression.type === utils_1.AST_NODE_TYPES.ChainExpression) {
                    expression = expression.expression;
                }
                const isUnhandled = isUnhandledFailure(checker, expression);
                if (isUnhandled) {
                    context.report({
                        node,
                        messageId: "noFloatingFailures",
                        suggest: [
                            {
                                messageId: "floatingFixVoid",
                                fix(fixer) {
                                    return [
                                        fixer.insertTextBefore(node, "void ("),
                                        fixer.insertTextAfterRange([expression.range[1], expression.range[1]], ")"),
                                    ];
                                },
                            },
                        ],
                    });
                }
            },
        };
        function isUnhandledFailure(checker, node) {
            // First, check expressions whose resulting types may not be failure-like
            if (node.type === utils_1.AST_NODE_TYPES.SequenceExpression) {
                // Any child in a comma expression could return a potentially unhandled
                // promise, so we check them all regardless of whether the final returned
                // value is failure-like.
                return node.expressions.some((item) => isUnhandledFailure(checker, item));
            }
            // Check the type. At this point it can't be unhandled if it isn't a failure
            if (!(0, is_failure_like_1.isSometimesFailureLike)(checker, services.getTypeAtLocation(node))) {
                return false;
            }
            if (node.type === utils_1.AST_NODE_TYPES.CallExpression) {
                return true;
            }
            else if (node.type === utils_1.AST_NODE_TYPES.AwaitExpression) {
                return true;
            }
            else if (node.type === utils_1.AST_NODE_TYPES.ConditionalExpression) {
                // We must be getting the promise-like value from one of the branches of the
                // ternary. Check them directly.
                const alternateResult = isUnhandledFailure(checker, node.alternate);
                if (alternateResult) {
                    return alternateResult;
                }
                return isUnhandledFailure(checker, node.consequent);
            }
            else if (node.type === utils_1.AST_NODE_TYPES.MemberExpression ||
                node.type === utils_1.AST_NODE_TYPES.Identifier ||
                node.type === utils_1.AST_NODE_TYPES.NewExpression) {
                // If it is just a property access chain or a `new` call (e.g. `foo.bar` or
                // `new Promise()`), the promise is not handled because it doesn't have the
                // necessary then/catch call at the end of the chain.
                return true;
            }
            else if (node.type === utils_1.AST_NODE_TYPES.LogicalExpression) {
                const leftResult = isUnhandledFailure(checker, node.left);
                if (leftResult) {
                    return leftResult;
                }
                return isUnhandledFailure(checker, node.right);
            }
            // Conservatively return false for all other expressions to avoid false positives
            // in cases we didn't consider.
            return false;
        }
    },
});
