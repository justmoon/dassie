/**
 * The scenarios (spec 6 §2): choosing one stops every node, wipes their
 * data and starts it; "Start over" does the same for the one running.
 */

import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";
import type { Scenario } from "@dassie/rpc/dev";
import { useRouter } from "@dassie/ui/router";
import { Badge, Card, ConfirmButton, PageTitle } from "@dassie/ui/ui";
import { useDev } from "../store.ts";

export function Scenarios(): JSX.Element {
  const dev = useDev();
  return (
    <div class="page-narrow stack" data-testid="page-scenarios">
      <PageTitle>Scenarios</PageTitle>
      <For each={dev.state().scenarios} keyed={(scenario) => scenario.id}>
        {(scenario) => (
          <ScenarioCard
            scenario={scenario()}
            current={scenario().id === dev.state().scenario}
          />
        )}
      </For>
    </div>
  );
}

function ScenarioCard(
  props: { scenario: Scenario; current: boolean },
): JSX.Element {
  const dev = useDev();
  const router = useRouter();
  const run = () => {
    dev.send({ case: "setScenario", value: { id: props.scenario.id } });
    router.navigate("/");
  };
  return (
    <Card
      class={props.current ? "scenario current" : "scenario"}
      testid={`scenario-${props.scenario.id}`}
    >
      <div class="scenario-head">
        <h2>{props.scenario.name}</h2>
        <Show when={props.current}>
          <Badge tone="good">running</Badge>
        </Show>
      </div>
      <p>{props.scenario.description}</p>
      <Show
        when={props.scenario.available}
        fallback={<p class="muted">Not yet: {props.scenario.unavailable}</p>}
      >
        <div class="actions">
          <Show
            when={props.current}
            fallback={
              <button
                type="button"
                class="primary"
                data-testid={`run-${props.scenario.id}`}
                onClick={run}
              >
                Run
              </button>
            }
          >
            <RunningActions />
          </Show>
        </div>
      </Show>
    </Card>
  );
}

/** What the scenario running offers: start over, or restart its nodes. */
function RunningActions(): JSX.Element {
  const dev = useDev();
  const router = useRouter();
  return (
    <>
      <ConfirmButton
        label="Start over"
        title="Start the scenario over?"
        confirm="Start over"
        tone="danger"
        testid="reset-scenario"
        onConfirm={() => {
          dev.send({ case: "reset", value: {} });
          router.navigate("/");
        }}
      >
        <p>Every node stops, loses its data and starts afresh.</p>
      </ConfirmButton>
      <button
        type="button"
        data-testid="restart-nodes"
        onClick={() => dev.send({ case: "restartNodes", value: {} })}
      >
        Restart the nodes
      </button>
    </>
  );
}
