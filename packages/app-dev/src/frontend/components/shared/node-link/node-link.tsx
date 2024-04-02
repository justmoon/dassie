import { Link } from "wouter"

import { COLORS } from "@dassie/app-node/src/frontend/constants/palette"
import { selectBySeed } from "@dassie/lib-logger"

const NodeLink = ({ id }: { id: string }) => (
  <Link style={{ color: selectBySeed(COLORS, id) }} href={`~/nodes/${id}`}>
    {id}
  </Link>
)

export default NodeLink
