import { Link } from "wouter"

import { COLORS } from "@dassie/gui-dassie/src/constants/palette"
import { selectBySeed } from "@dassie/lib-logger"

const NodeLink = ({ id }: { id: string }) => (
  <Link
    className="hover:underline"
    style={{ color: selectBySeed(COLORS, id) }}
    href={`~/nodes/${id}`}
  >
    {id}
  </Link>
)

export default NodeLink
