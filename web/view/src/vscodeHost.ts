// The VS Code webview's host: the extension sends models (projected by the
// language server) and copies of the sources they came from, each with its
// project and a revision; the model's ranges are on that revision. An edit
// names the revision it was made on and waits for its turn: one edit is in
// flight at a time. On its turn it is placed on the latest copy, through the
// edits applied since, and goes to the extension as offsets on that copy's
// revision. The extension applies it to the document the copy was read from,
// only while that copy is current, and answers with the revision the edited
// copy now has; the view updates when the server's fresh model arrives.
import { z } from "zod";
import type { EditResult, Host, NoteKind } from "./host.ts";
import { parseViewModel, type ViewModel } from "./model.ts";
import {
  historyOf,
  place,
  received,
  recorded,
  type FileHistory,
  type Revised,
  type SourceEdit,
} from "./revisions.ts";
import { TargetMap, targetOf, type ViewTarget } from "./target.ts";

const revisionSchema = z.int().nonnegative();

/** Messages from the extension. */
const inboundSchema = z.union([
  z.object({
    type: z.literal("model"),
    model: z.unknown(),
    /** The project root the model's file paths are relative to. */
    project: z.string(),
    sources: z.record(z.string(), z.string()),
    /** The revision of these copies, which the model's ranges are on. */
    revision: revisionSchema,
    main: z.boolean(),
  }),
  /** Applied: the edited copy is now at `revision`. */
  z.object({
    type: z.literal("edited"),
    id: z.int(),
    ok: z.literal(true),
    revision: revisionSchema,
  }),
  z.object({ type: z.literal("edited"), id: z.int(), ok: z.literal(false), message: z.string() }),
  z.object({ type: z.literal("note"), message: z.string().nullable() }),
]);
type Inbound = z.infer<typeof inboundSchema>;

/** Messages to the extension. */
type Outbound =
  | { type: "ready" }
  | {
      type: "edit";
      id: number;
      project: string;
      file: string;
      /** The revision of the copy the offsets are on: the latest the view holds. */
      revision: number;
      start: number;
      end: number;
      text: string;
    }
  /** Ask for the model of a target the view has none of yet: a module alone, or an include instance of a root. */
  | { type: "request"; target: ViewTarget };

interface VsCodeApi {
  postMessage(message: Outbound): void;
}

declare function acquireVsCodeApi(): VsCodeApi;

/** Whether this page runs inside a VS Code webview. */
export function inVsCode(): boolean {
  return typeof acquireVsCodeApi === "function";
}

type Answer = { ok: true; revision: number } | { ok: false; message: string };

export class VsCodeHost implements Host {
  readonly modules: readonly string[] = [];
  readonly edited = false;
  private readonly api = acquireVsCodeApi();
  private readonly models = new TargetMap<ViewModel>();
  /** The project each model came from (its file paths are relative to that root) and its revision. */
  private readonly origins = new WeakMap<ViewModel, { project: string; revision: number }>();
  /** The copy of each file, by project and then by file: two projects may both have `src/main.gcl`. */
  private readonly copies = new Map<string, Map<string, FileHistory>>();
  /** Edits sent and not answered yet, by id. */
  private readonly pending = new Map<number, (answer: Answer) => void>();
  private edits = 0;
  /** The edit whose turn it is, or the last one; the next waits for it. */
  private turn: Promise<void> = Promise.resolve();
  private readonly modelListeners: ((models: ViewModel[]) => void)[] = [];
  private readonly openListeners: ((module: string) => void)[] = [];
  private readonly noteListeners: ((message: string | null, kind: NoteKind) => void)[] = [];
  /** The module shown as the main sheet; a fresh model of it updates in place. */
  private mainModule: string | null = null;

  constructor() {
    window.addEventListener("message", (event: MessageEvent<unknown>) => this.receive(event.data));
    this.api.postMessage({ type: "ready" });
  }

  private receive(raw: unknown): void {
    const parsed = inboundSchema.safeParse(raw);
    if (!parsed.success) {
      // An edit's answer that does not read still ends the wait for it.
      const id = typeof raw === "object" && raw !== null ? (raw as { id?: unknown }).id : null;
      const settle = typeof id === "number" ? this.pending.get(id) : undefined;
      if (typeof id === "number") this.pending.delete(id);
      const message = "the extension sent a message this view cannot read; update both";
      settle?.({ ok: false, message });
      this.note(message);
      return;
    }
    this.take(parsed.data);
  }

  private take(message: Inbound): void {
    switch (message.type) {
      case "model": {
        let model: ViewModel;
        try {
          model = parseViewModel(message.model);
        } catch (error) {
          this.note(error instanceof Error ? error.message : String(error));
          return;
        }
        this.models.set(targetOf(model), model);
        this.origins.set(model, { project: message.project, revision: message.revision });
        const copies = this.copies.get(message.project) ?? new Map<string, FileHistory>();
        this.copies.set(message.project, copies);
        for (const [file, text] of Object.entries(message.sources)) {
          const history = copies.get(file);
          copies.set(
            file,
            history === undefined
              ? historyOf(text, message.revision)
              : received(history, text, message.revision),
          );
        }
        if (message.main && model.target.kind === "module" && this.mainModule !== model.module.id) {
          this.mainModule = model.module.id;
          for (const listener of this.openListeners) listener(model.module.id);
        } else {
          for (const listener of this.modelListeners) listener([model]);
        }
        return;
      }
      case "edited": {
        const settle = this.pending.get(message.id);
        this.pending.delete(message.id);
        settle?.(
          message.ok
            ? { ok: true, revision: message.revision }
            : { ok: false, message: message.message },
        );
        return;
      }
      case "note":
        this.note(message.message);
        return;
    }
  }

  private note(message: string | null): void {
    for (const listener of this.noteListeners) listener(message, "error");
  }

  model(target: ViewTarget): ViewModel | null {
    return this.models.get(target) ?? null;
  }

  source(model: ViewModel, file: string): Revised | null {
    const origin = this.origins.get(model);
    const history = origin === undefined ? undefined : this.copies.get(origin.project)?.get(file);
    return history === undefined ? null : { text: history.text, revision: history.revision };
  }

  projectedAt(model: ViewModel): number | null {
    return this.origins.get(model)?.revision ?? null;
  }

  edit(model: ViewModel, edit: SourceEdit): Promise<EditResult> {
    const result = this.turn.then(() => this.send(model, edit));
    this.turn = result.then(
      () => undefined,
      () => undefined,
    );
    return result;
  }

  /** On the edit's turn: place it on the latest copy and wait for the extension's answer. */
  private async send(model: ViewModel, edit: SourceEdit): Promise<EditResult> {
    const project = this.origins.get(model)?.project;
    const copies = project === undefined ? undefined : this.copies.get(project);
    const history = copies?.get(edit.file);
    if (project === undefined || copies === undefined || history === undefined) {
      return { ok: false, message: `unknown file ${edit.file}` };
    }
    const placed = place(history, edit);
    if (!placed.ok) return placed;
    const { change } = placed;
    this.edits += 1;
    const id = this.edits;
    const answer = await new Promise<Answer>((resolve) => {
      this.pending.set(id, resolve);
      this.api.postMessage({
        type: "edit",
        id,
        project,
        file: edit.file,
        revision: history.revision,
        start: change.from,
        end: change.to,
        text: change.insert,
      });
    });
    if (!answer.ok) return answer;
    const next = recorded(history, change, answer.revision);
    // Applied: the copy takes the edit, unless a fresh copy came first.
    if (copies.get(edit.file) === history) copies.set(edit.file, next);
    return {
      ok: true,
      models: [],
      note: null,
      source: { text: next.text, revision: next.revision },
    };
  }

  request(target: ViewTarget): void {
    this.api.postMessage({ type: "request", target });
  }

  reset(): void {
    // The editor owns the sources; there is nothing to discard here.
  }

  onModels(listener: (models: ViewModel[]) => void): void {
    this.modelListeners.push(listener);
  }

  onOpen(listener: (module: string) => void): void {
    this.openListeners.push(listener);
  }

  onNote(listener: (message: string | null, kind: NoteKind) => void): void {
    this.noteListeners.push(listener);
  }
}
