"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.isTopLevel = isTopLevel;
const types_1 = require("@typescript-eslint/types");
function isTopLevel(node) {
    let scope = node.parent;
    while (scope?.type === types_1.AST_NODE_TYPES.BlockStatement) {
        scope = scope.parent;
    }
    return scope?.type === types_1.AST_NODE_TYPES.Program;
}
