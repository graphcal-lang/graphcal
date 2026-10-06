// Scripts injected into the sandboxed report frame. They are only needed once a
// report renders, so `report.ts` loads this module on demand, keeping them out
// of the playground's entry bundle.
import formState from "../../../crates/graphcal-report/src/report_form_state.js?raw";
import runtime from "../../../crates/graphcal-report/src/report_runtime.js?raw";
import outlineState from "../../../crates/graphcal-report/src/report_outline_state.js?raw";
import workspace from "../../../crates/graphcal-report/src/report_workspace.js?raw";
import results from "../../../crates/graphcal-report/src/report_results.js?raw";
import charts from "../../../crates/graphcal-report/src/report_charts.js?raw";
import bootstrap from "./report-frame.js?raw";

export const reportAssets = {
  formState,
  outlineState,
  results,
  workspace,
  runtime,
  charts,
  bootstrap,
};

export type ReportAssets = typeof reportAssets;
