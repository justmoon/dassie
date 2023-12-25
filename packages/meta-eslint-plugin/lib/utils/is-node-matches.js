"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.isNodeMatches = exports.isNodeMatchesNameOrPath = void 0;
/**
 * Check if node matches object name or key path.
 *
 * @param {Node} node - The AST node to check.
 * @param {string} nameOrPath - The object name or key path.
 * @returns {boolean}
 **/
function isNodeMatchesNameOrPath(node, nameOrPath) {
    const names = nameOrPath.trim().split(".");
    for (let index = names.length - 1; index >= 0; index--) {
        const name = names[index];
        if (!name) {
            return false;
        }
        if (index === 0) {
            return ((node.type === "Identifier" && node.name === name) ||
                (name === "this" && node.type === "ThisExpression"));
        }
        if (node.type !== "MemberExpression" ||
            node.optional ||
            node.computed ||
            node.property.type !== "Identifier" ||
            node.property.name !== name) {
            return false;
        }
        node = node.object;
    }
    throw new Error("Unreachable");
}
exports.isNodeMatchesNameOrPath = isNodeMatchesNameOrPath;
/**
 * Check if node matches any object name or key path.
 *
 * @param {Node} node - The AST node to check.
 * @param {string[]} nameOrPaths - The object name or key paths.
 * @returns {boolean}
 **/
function isNodeMatches(node, nameOrPaths) {
    return nameOrPaths.some((nameOrPath) => isNodeMatchesNameOrPath(node, nameOrPath));
}
exports.isNodeMatches = isNodeMatches;
