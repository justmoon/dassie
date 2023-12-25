"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.rule = void 0;
const create_rule_1 = require("../utils/create-rule");
const is_top_level_1 = require("../utils/is-top-level");
exports.rule = (0, create_rule_1.createRule)({
    name: "no-top-level-mutables",
    meta: {
        docs: {
            description: "There should be no mutable state outside of functions.",
            recommended: "recommended",
        },
        messages: {
            noTopLevelMutable: "Using {{clause}} at the top level creates global, mutable state. Wrap your state in a factory function so that it is scoped to a specific instance of your code.",
        },
        schema: [],
        type: "problem",
    },
    defaultOptions: [],
    create(context) {
        return {
            VariableDeclaration(node) {
                if ((node.kind === "let" || node.kind === "var") && (0, is_top_level_1.isTopLevel)(node)) {
                    context.report({
                        node,
                        messageId: "noTopLevelMutable",
                        data: { clause: node.kind },
                    });
                }
            },
        };
    },
});
