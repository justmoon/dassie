import { Link } from "wouter"

import { selectBySeed } from "@dassie/lib-logger"

import { COLORS } from "../../../constants/palette"

const NodeLink = ({ id }: { id: string }) => (
  <Link style={{ color: selectBySeed(COLORS, id) }} href={`/nodes/${id}`}>
    {id}
  </Link>
)

export default NodeLink
