"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.isTopLevel = void 0;
function isTopLevel(node) {
    let scope = node.parent;
    while (scope?.type === "BlockStatement") {
        scope = scope.parent;
    }
    return scope?.type === "Program";
}
exports.isTopLevel = isTopLevel;
