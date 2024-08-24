"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.isTopLevel = isTopLevel;
function isTopLevel(node) {
    let scope = node.parent;
    while (scope?.type === "BlockStatement") {
        scope = scope.parent;
    }
    return scope?.type === "Program";
}
