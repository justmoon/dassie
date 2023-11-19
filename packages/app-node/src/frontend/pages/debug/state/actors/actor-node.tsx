import { combine } from "../../../../utils/class-helper"

interface ActorNodeProperties {
  id: number
  actorTree: Map<number, number[]>
  names: Map<number, string>
}
export function ActorNode({ id, actorTree, names }: ActorNodeProperties) {
  const name = names.get(id)
  const children = actorTree.get(id)

  return (
    <li className={combine("rounded-md p-2")}>
      {name}
      <ul className="pl-2">
        {children?.map((childId) => (
          <ActorNode
            key={childId}
            id={childId}
            actorTree={actorTree}
            names={names}
          />
        ))}
      </ul>
    </li>
  )
}
