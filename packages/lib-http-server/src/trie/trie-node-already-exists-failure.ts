import { Failure } from "@dassie/lib-type-utils"

export class TrieNodeAlreadyExistsFailure extends Failure {
  override name = "TrieNodeAlreadyExistsFailure"
}

export const TRIE_NODE_ALREADY_EXISTS_FAILURE =
  new TrieNodeAlreadyExistsFailure()
