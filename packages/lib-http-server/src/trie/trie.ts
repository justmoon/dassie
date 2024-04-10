import { arrayEquals, isFailure } from "@dassie/lib-type-utils"

import {
  TRIE_NODE_ALREADY_EXISTS_FAILURE,
  type TrieNodeAlreadyExistsFailure,
} from "./trie-node-already-exists-failure"

type TrieLeaf<T> = {
  value: T
  path: readonly string[]
}

type TrieNode<T> = Map<string, TrieNode<T> | TrieLeaf<T>>

/**
 * The segment wildcard matches a single path segment.
 */
export const SEGMENT_WILDCARD = "?"

/**
 * The prefix wildcard matches all remaining path segments and must appear at
 * the end of a path only.
 */
export const PREFIX_WILDCARD = "*"

export class Trie<T> {
  readonly root: TrieNode<T> = new Map()

  insert(path: readonly string[], value: T) {
    const prefixWildcardIndex = path.indexOf(PREFIX_WILDCARD)

    if (prefixWildcardIndex !== -1 && prefixWildcardIndex !== path.length - 1) {
      throw new Error("Prefix wildcard must be at the end of the path")
    }

    return this.insertAtNode(this.root, path, 0, value)
  }

  /**
   * Update a trie node to insert a value at a given path.
   *
   * @param node - The node to insert the value at.
   * @param path - The path to insert the value at.
   * @param pathIndex - The current index in the path.
   * @param value - The value to insert.
   * @returns Undefined if the value was inserted successfully, or a TrieNodeAlreadyExistsFailure if the node already exists.
   */
  private insertAtNode(
    node: TrieNode<T>,
    path: readonly string[],
    pathIndex: number,
    value: T,
  ): void | TrieNodeAlreadyExistsFailure {
    if (path.length === pathIndex) {
      if (node.has("")) {
        return TRIE_NODE_ALREADY_EXISTS_FAILURE
      }

      node.set("", { value, path })
      return
    }

    const head = path[pathIndex]!

    const nextNode = node.get(head)

    if (nextNode === undefined) {
      node.set(head, { value, path })
    } else if (nextNode instanceof Map) {
      return this.insertAtNode(nextNode, path, pathIndex + 1, value)
    } else {
      const isOldNodeExactMatch = nextNode.path.length === pathIndex + 1
      const isNewNodeExactMatch = path.length === pathIndex + 1

      if (isOldNodeExactMatch && isNewNodeExactMatch) {
        return TRIE_NODE_ALREADY_EXISTS_FAILURE
      }

      const newBranch: TrieNode<T> = new Map()

      if (isOldNodeExactMatch) {
        newBranch.set("", nextNode)
      } else {
        newBranch.set(nextNode.path[pathIndex + 1]!, nextNode)
      }

      const result = this.insertAtNode(newBranch, path, pathIndex + 1, value)

      if (isFailure(result)) {
        return result
      }

      node.set(head, newBranch)
    }
  }

  remove(path: readonly string[]) {
    if (path.length === 0) {
      throw new Error("Path cannot be empty")
    }

    this.removeAtNode(this.root, path, 0)
  }

  private removeAtNode(
    node: TrieNode<T>,
    path: readonly string[],
    pathIndex: number,
  ) {
    if (path.length === pathIndex) {
      node.delete("")
      return
    }

    const head = path[pathIndex]!

    const nextNode = node.get(head)

    if (nextNode === undefined) {
      return
    }

    if (nextNode instanceof Map) {
      const newLeaf = this.getLastLeaf(nextNode, path)

      if (newLeaf) {
        node.set(head, newLeaf)
        return
      }

      this.removeAtNode(nextNode, path, pathIndex + 1)
    } else {
      if (arrayEquals(nextNode.path, path)) {
        node.delete(head)
      }
    }
  }

  /**
   * Checks a node. If the node has exactly two children, they are both leaves,
   * and one of them is about to be removed, returns the other leaf so the node
   * can be replaced with it. Otherwise, returns false.
   */
  private getLastLeaf(
    node: TrieNode<T>,
    removalPath: readonly string[],
  ): TrieLeaf<T> | false {
    if (node.size !== 2) {
      return false
    }

    const [first, second] = [...node.values()] as [
      TrieNode<T> | TrieLeaf<T>,
      TrieNode<T> | TrieLeaf<T>,
    ]

    if (first instanceof Map || second instanceof Map) {
      return false
    }

    if (arrayEquals(first.path, removalPath)) {
      return second
    }

    if (arrayEquals(second.path, removalPath)) {
      return first
    }

    return false
  }

  get(path: readonly string[]): T | undefined {
    return this.getAtNode(this.root, path, 0)?.value
  }

  private getAtNode(
    node: TrieNode<T>,
    path: readonly string[],
    pathIndex: number,
  ): TrieLeaf<T> | undefined {
    if (path.length === pathIndex) {
      const leaf = node.get("")

      if (leaf instanceof Map) {
        // eslint-disable-next-line unicorn/prefer-type-error
        throw new Error("Redundant trie node found")
      }

      if (leaf) return leaf

      const prefixWildcardNode = node.get(PREFIX_WILDCARD)

      if (prefixWildcardNode instanceof Map) {
        // eslint-disable-next-line unicorn/prefer-type-error
        throw new Error("Prefix wildcard must be at the end of the path")
      }

      return prefixWildcardNode
    }

    const head = path[pathIndex]!

    const nextNode = node.get(head)

    if (nextNode instanceof Map) {
      const result = this.getAtNode(nextNode, path, pathIndex + 1)

      if (result !== undefined) {
        return result
      }
    } else if (nextNode && this.pathEquals(nextNode.path, path)) {
      return nextNode
    }

    const segmentWildcardNode = node.get(SEGMENT_WILDCARD)

    if (segmentWildcardNode instanceof Map) {
      return this.getAtNode(segmentWildcardNode, path, pathIndex + 1)
    } else if (
      segmentWildcardNode &&
      this.pathEquals(segmentWildcardNode.path, path)
    ) {
      return segmentWildcardNode
    }

    const prefixWildcardNode = node.get(PREFIX_WILDCARD)

    if (prefixWildcardNode instanceof Map) {
      // eslint-disable-next-line unicorn/prefer-type-error
      throw new Error("Prefix wildcard must be at the end of the path")
    } else if (prefixWildcardNode) {
      return prefixWildcardNode
    }

    return undefined
  }

  /**
   * Check if a target path matches a given path pattern.
   *
   * The pattern can contain segment wildcards, which match a single path
   * segment or a prefix wildcard at the end, which matches all remaining
   * path segments.
   */
  private pathEquals(pattern: readonly string[], target: readonly string[]) {
    const isPrefix = pattern.at(-1) === PREFIX_WILDCARD
    if (isPrefix) {
      if (pattern.length - 1 > target.length) {
        return false
      }

      for (let index = 0; index < pattern.length - 1; index++) {
        if (pattern[index] !== target[index]) {
          return false
        }
      }

      return true
    }

    return (
      pattern.length === target.length &&
      pattern.every(
        (value, index) =>
          target[index] === value || pattern[index] === SEGMENT_WILDCARD,
      )
    )
  }
}
