import { z } from "zod";
import formState from "../../../crates/graphcal-report/src/report_form_state.js?raw";
import runtime from "../../../crates/graphcal-report/src/report_runtime.js?raw";
import bootstrap from "./report-frame.js?raw";
import { bindingsSchema, type Binding } from "./bindings";
import type { ReportOutcome, ParameterPort } from "./protocol";

const messageSchema = z.discriminatedUnion("type", [
  z.object({ type: z.literal("ready"), session: z.string() }),
  z.object({ type: z.literal("pending"), session: z.string() }),
  z.object({
    type: z.literal("evaluate"),
    session: z.string(),
    id: z.int().positive(),
    bindings: z.unknown(),
  }),
]);

function scriptJson(value: unknown) {
  return JSON.stringify(value).replaceAll("<", "\\u003c");
}

/** Source/session checks are required: every sandboxed srcdoc has origin "null". */
export class Report {
  private frame?: HTMLIFrameElement;
  private session = "";
  private request?: { id: number; current: boolean };
  private latestId = 0;
  private listener?: (event: MessageEvent<unknown>) => void;
  private readonly container: HTMLElement;
  private readonly evaluate: (bindings: Binding[]) => void;
  private readonly pending: () => void;

  constructor(
    container: HTMLElement,
    evaluate: (bindings: Binding[]) => void,
    pending: () => void,
  ) {
    this.container = container;
    this.evaluate = evaluate;
    this.pending = pending;
  }

  clear(message = "Run to generate an interactive report.") {
    if (this.listener) window.removeEventListener("message", this.listener);
    this.listener = undefined;
    this.frame?.remove();
    this.frame = undefined;
    this.session = "";
    this.request = undefined;
    this.latestId = 0;
    const notice = document.createElement("p");
    notice.textContent = message;
    this.container.replaceChildren(notice);
  }

  render(
    outcome: Extract<ReportOutcome, { status: "evaluated" }>,
    ports: ParameterPort[],
    bindings: Binding[],
  ) {
    this.clear();
    const frame = document.createElement("iframe");
    this.frame = frame;
    this.session = crypto.randomUUID();
    frame.title = "Interactive Graphcal report";
    frame.setAttribute("sandbox", "allow-scripts");
    frame.referrerPolicy = "no-referrer";
    const session = this.session;
    const origin = window.location.origin;
    this.listener = (event) => {
      if (event.source !== frame.contentWindow || event.origin !== "null") return;
      const parsed = messageSchema.safeParse(event.data);
      if (!parsed.success || parsed.data.session !== this.session) return;
      const message = parsed.data;
      switch (message.type) {
        case "ready":
          this.send({ type: "initialize", ports, evaluation: outcome.evaluation, bindings });
          break;
        case "pending":
          if (this.request) this.request.current = false;
          this.pending();
          break;
        case "evaluate": {
          if (message.id <= this.latestId) return;
          this.latestId = message.id;
          const checked = bindingsSchema.safeParse(message.bindings);
          if (!checked.success) {
            this.send({
              type: "result",
              id: message.id,
              outcome: {
                status: "eval_error",
                message:
                  "Parameter inputs exceed browser limits or contain duplicate names. Showing last successful evaluation.",
              },
            });
            return;
          }
          this.request = { id: message.id, current: true };
          this.evaluate(checked.data);
          break;
        }
      }
    };
    window.addEventListener("message", this.listener);
    const assets = new URL(`${import.meta.env.BASE_URL}vega/`, origin).href;
    const policy = `default-src 'none'; script-src 'unsafe-inline' 'unsafe-eval' ${assets}; style-src 'unsafe-inline'; img-src data: blob:; base-uri 'none'; form-action 'none'`;
    const needsCharts =
      outcome.evaluation.figures.length > 0 ||
      outcome.evaluation.notices.some((notice) => notice.kind === "plot_error");
    const scripts = (needsCharts ? ["vega", "vega-lite", "vega-embed"] : [])
      .map((name) => `<script src="${assets}${name}.min.js"></script>`)
      .join("");
    frame.srcdoc = outcome.html
      .replace(
        "<head>",
        () =>
          `<head><meta http-equiv="Content-Security-Policy" content="${policy}">${scripts}<script>${formState}</script><script>${runtime}</script><script>${bootstrap}</script>`,
      )
      .replace(
        "</body>",
        () =>
          `<script id="report-connection" type="application/json">${scriptJson({ session, origin })}</script></body>`,
      );
    this.container.replaceChildren(frame);
  }

  /** Forward even superseded replies so the runtime can drain its serialized queue. */
  deliver(outcome: ReportOutcome): "initial" | "current" | "stale" {
    if (!this.request) return "initial";
    const request = this.request;
    this.request = undefined;
    this.send({ type: "result", id: request.id, outcome });
    return request.current ? "current" : "stale";
  }

  private send(message: object) {
    // A sandboxed frame cannot be addressed by an origin other than "*".
    this.frame?.contentWindow?.postMessage({ ...message, session: this.session }, "*");
  }
}
