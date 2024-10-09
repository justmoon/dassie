import { createActor } from "@dassie/lib-reactive"

import type { DassieActorContext } from "../base/types/dassie-base"
import { AddMajorityNodesActor } from "./add-majority-nodes"
import { BroadcastStateUpdatesActor } from "./broadcast-state-updates"
import { CreatePeerLedgerEntriesActor } from "./create-peer-ledger-entries"
import { DownloadNodeListsActor } from "./download-node-lists"
import { ForwardLinkStateUpdateActor } from "./forward-link-state-update"
import { MaintainOwnNodeTableEntryActor } from "./maintain-own-node-table-entry"
import { MaintainPeeringRelationshipsActor } from "./maintain-peering-relationships"
import { PersistNodeTableActor } from "./persist-node-table"
import { PollNodeListHashesActor } from "./poll-node-list-hashes"
import { ReceiveAddressFromUplinkActor } from "./receive-address-from-uplink"
import { RefreshNodeStateActor } from "./refresh-node-state"
import { RegisterPeerHttpHandlerActor } from "./register-peer-http-handler"
import { SendHeartbeatsActor } from "./send-heartbeats"

export const PeerProtocolActor = () =>
  createActor(async (sig: DassieActorContext) => {
    sig.run(PersistNodeTableActor)
    sig.run(RegisterPeerHttpHandlerActor)

    await sig.run(MaintainOwnNodeTableEntryActor)
    sig.run(MaintainPeeringRelationshipsActor)

    await sig.run(SendHeartbeatsActor)
    sig.run(ForwardLinkStateUpdateActor)
    await sig.run(PollNodeListHashesActor)
    await sig.run(DownloadNodeListsActor)
    sig.run(AddMajorityNodesActor)
    sig.run(BroadcastStateUpdatesActor)
    await sig.run(RefreshNodeStateActor)
    sig.runMap(ReceiveAddressFromUplinkActor)

    sig.runMap(CreatePeerLedgerEntriesActor)
  })
