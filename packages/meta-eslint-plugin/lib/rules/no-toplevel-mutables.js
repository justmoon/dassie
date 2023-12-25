"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.rule = void 0;
const create_rule_1 = require("../utils/create-rule");
exports.rule = (0, create_rule_1.createRule)({
    name: "no-toplevel-mutables",
    meta: {
        docs: {
            description: "There should be no mutable state outside of functions.",
            recommended: "recommended",
        },
        messages: {
            noToplevelMutable: "Using {{clause}} at the top level creates global, mutable state. Wrap your state in a factory function so that it is scoped to a specific instance of your code.",
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
                        messageId: "noToplevelMutable",
                        data: { clause: node.kind },
                    });
                }
            },
        };
        function isTopLevel(node) {
            let scope = node.parent;
            while (scope.type === "BlockStatement") {
                scope = scope.parent;
            }
            return scope.type === "Program";
        }
    },
});
