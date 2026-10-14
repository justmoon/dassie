/**
 * The scenarios (spec 6 §2): choosing one stops every node, wipes their
 * data and starts it; "Start over" does the same for the one running.
 */

import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";
import { useRouter } from "@dassie/ui/router";
import { Badge, Card, ConfirmButton, PageTitle } from "@dassie/ui/ui";
import { useDev } from "../store.ts";

export function Scenarios(): JSX.Element {
  const dev = useDev();
  const router = useRouter();
  const current = () => dev.state().scenario;
  return (
    <div class="page-narrow stack" data-testid="page-scenarios">
      <PageTitle>Scenarios</PageTitle>
      <For each={dev.state().scenarios} keyed={(scenario) => scenario.id}>
        {(scenario) => (
          <Card
            class={scenario().id === current()
              ? "scenario current"
              : "scenario"}
            testid={`scenario-${scenario().id}`}
          >
            <div class="scenario-head">
              <h2>{scenario().name}</h2>
              <Show when={scenario().id === current()}>
                <Badge tone="good">running</Badge>
              </Show>
            </div>
            <p>{scenario().description}</p>
            <Show
              when={scenario().available}
              fallback={<p class="muted">Not yet: {scenario().unavailable}</p>}
            >
              <div class="actions">
                <Show
                  when={scenario().id === current()}
                  fallback={
                    <button
                      type="button"
                      class="primary"
                      data-testid={`run-${scenario().id}`}
                      onClick={() => {
                        dev.send({
                          case: "setScenario",
                          value: { id: scenario().id },
                        });
                        router.navigate("/");
                      }}
                    >
                      Run
                    </button>
                  }
                >
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
                    onClick={() =>
                      dev.send({ case: "restartNodes", value: {} })}
                  >
                    Restart the nodes
                  </button>
                </Show>
              </div>
            </Show>
          </Card>
        )}
      </For>
    </div>
  );
}
