// Exports the ViewModel fixtures with `graphcal view` from the sample projects
// under fixtures/<name>_project. Uses the graphcal binary on PATH, or the
// workspace's debug build when GRAPHCAL points at it.
import { execFileSync } from "node:child_process";
import { writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const graphcal = process.env.GRAPHCAL ?? "graphcal";
const fixtures = [
  ["power_max", "power_project/src/power_project/power_max.gcl"],
  ["eps", "power_project/src/power_project/eps.gcl"],
  ["components", "power_project/src/power_project/components.gcl"],
  ["power_budget", "power_budget_project/src/power_budget/power_budget.gcl"],
];

for (const [name, file] of fixtures) {
  const json = execFileSync(graphcal, ["view", join(here, file)], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"],
  });
  const model = JSON.parse(json);
  writeFileSync(join(here, `${name}.view.json`), `${JSON.stringify(model, null, 2)}\n`);
  console.log(`wrote fixtures/${name}.view.json (${model.nodes.length} nodes)`);
}
