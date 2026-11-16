import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useBackend } from "../backend-context";
import {
  type DraftColumn,
  ITEM_TYPES,
  ITEM_TYPE_LABELS,
  MODE_LABELS,
  addSection,
  buildColumns,
  certaintyMark,
  dropBefore,
  formatClock,
  isIsoDate,
  isUnverified,
  matchPercent,
  moveSection,
  moveTargets,
  orphanedItems,
  removeSection,
  renameSection,
  sectionsError,
  usesSections,
} from "../draft";
import { errorMessage, useCycleFacts, useOutbox } from "../hooks";
import { isIpcError } from "../ipc/tauri";
import type {
  DraftItem,
  DraftSection,
  DraftView,
  ItemType,
  RenderVariant,
  ReportDefaults,
  ReportMode,
} from "../ipc/types";
import { lockedByOps, opsOf } from "../publish";
import { MODE_HELP, MODE_NAMES, REPORT_MODES } from "../report-gate";
import { MarkdownView } from "./MarkdownView";
import { BuildDraftDialog } from "./ProcessDialogs";
import { CorrectionsPanel } from "./CorrectionsPanel";
import { PublishPanel } from "./PublishPanel";
import { StatusProposals } from "./StatusProposals";

type Pane = "items" | "report";

/**
 * The editor of the draft report of a meeting: the sections with their items, each item with its
 * quote and a jump to the transcript, the report mode, and the rendered report. Every change goes
 * to the backend and the view it answers with replaces what is shown.
 */
export function DraftEditor(props: {
  meetingId: string;
  onBack: () => void;
  /** Opens the transcript at a time in the meeting. */
  onOpenTranscript: (atMs: number) => void;
  onOpenQueue: () => void;
}) {
  const { meetingId, onBack, onOpenTranscript, onOpenQueue } = props;
  const backend = useBackend();
  const [view, setView] = useState<DraftView | null>(null);
  const [noDraft, setNoDraft] = useState(false);
  const [title, setTitle] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [pane, setPane] = useState<Pane>("items");
  const [defaults, setDefaults] = useState<ReportDefaults | null>(null);
  const [sourceOf, setSourceOf] = useState<string | null>(null);
  const [editingSections, setEditingSections] = useState(false);
  const [building, setBuilding] = useState(false);
  const [dragId, setDragId] = useState<string | null>(null);

  const load = useCallback(() => {
    backend
      .getDraft(meetingId)
      .then((v) => {
        setNoDraft(false);
        setView(v);
      })
      .catch((e) => {
        if (isIpcError(e) && e.code === "no_draft") setNoDraft(true);
        else setError(errorMessage(e));
      });
  }, [backend, meetingId]);

  useEffect(() => {
    load();
    void backend
      .getMeeting(meetingId)
      .then((m) => setTitle(m.summary.title))
      .catch(() => undefined);
    let off: (() => void) | undefined;
    let cancelled = false;
    void backend
      .onDraftChanged((id) => id === meetingId && load())
      .then((u) => {
        if (cancelled) u();
        else off = u;
      });
    return () => {
      cancelled = true;
      off?.();
    };
  }, [backend, meetingId, load]);

  // Publishing locks the draft and cancelling the operations unlocks it: neither changes the
  // draft itself, so the view is loaded again when the outbox says the lock changed.
  const outbox = useOutbox();
  const lockedByOutbox = outbox === null ? null : lockedByOps(opsOf(outbox, meetingId));
  const seenLock = useRef<boolean | null>(null);
  useEffect(() => {
    if (lockedByOutbox === null) return;
    if (seenLock.current !== null && seenLock.current !== lockedByOutbox) load();
    seenLock.current = lockedByOutbox;
  }, [lockedByOutbox, load]);

  const cycleId = view?.draft.cycle;
  const { tasks, topics } = useCycleFacts(cycleId);
  useEffect(() => {
    if (!cycleId) return;
    let cancelled = false;
    backend
      .reportDefaults(cycleId)
      .then((d) => !cancelled && setDefaults(d))
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [backend, cycleId]);

  // A click outside the open source popover closes it.
  useEffect(() => {
    if (sourceOf === null) return;
    const onDown = (e: MouseEvent) => {
      if (!(e.target instanceof Element) || !e.target.closest("[data-source-root]"))
        setSourceOf(null);
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [sourceOf]);

  /** Runs a change that answers with the new view. */
  const apply = useCallback((change: Promise<DraftView>): Promise<boolean> => {
    setError(null);
    return change
      .then((v) => {
        setView(v);
        return true;
      })
      .catch((e) => {
        setError(errorMessage(e));
        return false;
      });
  }, []);

  const columns = useMemo(() => (view ? buildColumns(view.draft) : []), [view]);

  const header = (
    <div className="page-head">
      <h2>
        <span className="glitch" data-text="Draft">
          Draft
        </span>
      </h2>
      <span className="head-actions">
        <button className="btn btn-small" onClick={() => onOpenTranscript(0)}>
          Transcript
        </button>
        <button className="btn btn-small btn-ghost" onClick={onBack}>
          Back to meetings
        </button>
      </span>
    </div>
  );

  if (noDraft) {
    return (
      <>
        {header}
        {title && <p className="card-when">{title}</p>}
        <p className="empty">
          There is no draft for this meeting yet. The draft is made from the transcript by a
          language model, in the processing stages Extract items and Build draft.
        </p>
        <div className="card-actions" style={{ justifyContent: "flex-start" }}>
          <button className="btn btn-record" onClick={() => setBuilding(true)}>
            Build draft…
          </button>
          <button className="btn btn-ghost" onClick={onOpenQueue}>
            Open the queue
          </button>
        </div>
        {building && (
          <BuildDraftDialog
            meetingId={meetingId}
            onClose={() => setBuilding(false)}
            onStarted={() => {
              setBuilding(false);
              onOpenQueue();
            }}
          />
        )}
        {error && (
          <p className="error" role="alert">
            {error}
          </p>
        )}
      </>
    );
  }

  if (!view) {
    return (
      <>
        {header}
        {error && (
          <p className="error" role="alert">
            {error}
          </p>
        )}
      </>
    );
  }

  const { draft } = view;
  const people = view.participants;
  const locked = view.locked;
  const movable = usesSections(draft);

  const changeMode = (mode: ReportMode) => {
    const template =
      mode === "template" ? (draft.template ?? defaults?.defaultTemplate ?? undefined) : undefined;
    const meetingType =
      mode === "by-type"
        ? (draft.meetingType ?? defaults?.defaultMeetingType ?? undefined)
        : undefined;
    void apply(backend.setReportMode(meetingId, mode, template, meetingType));
  };

  const addItem = (type: ItemType, text: string, sectionId: string | null) =>
    apply(backend.addDraftItem(meetingId, { type, text, section: sectionId }));

  const dropOn = (column: DraftColumn, over: DraftItem | null) => {
    const moving = draft.items.find((i) => i.id === dragId);
    setDragId(null);
    if (!moving || column.id === null) return;
    void apply(
      backend.moveDraftItem(meetingId, moving.id, column.id, dropBefore(column.id, over, moving)),
    );
  };

  return (
    <>
      {header}
      {title && <p className="card-when">{title}</p>}
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}

      {view.hasNewDraft && (
        <div className="banner" role="status">
          <span>
            <strong>A newer draft is available.</strong> Processing ran again and produced a new
            draft. Yours, with your edits, stays until you decide.
          </span>
          <span className="banner-actions">
            <button
              className="btn btn-small btn-record"
              disabled={locked}
              onClick={() => void apply(backend.acceptNewDraft(meetingId))}
            >
              Accept new draft
            </button>
            <button
              className="btn btn-small btn-ghost"
              disabled={locked}
              onClick={() => void apply(backend.discardNewDraft(meetingId))}
            >
              Discard new draft
            </button>
          </span>
        </div>
      )}
      {view.reportWarning && <p className="notice">{view.reportWarning}</p>}
      {locked && (
        <div className="banner banner-locked" role="status">
          <span>
            <strong>This draft is locked.</strong> The meeting was published or queued for
            publishing, so the draft stays as it was sent. Cancel the queued operations in the
            publishing panel below to edit it again.
          </span>
        </div>
      )}

      <div className="draft-bar">
        <span className="chip">{draft.cycle}</span>
        <span className="chip">{draft.date}</span>
        <span className="chip">Language: {draft.language}</span>
        <label className="inline-field">
          Report mode
          <select
            aria-label="Report mode"
            value={draft.mode}
            disabled={locked}
            onChange={(e) => changeMode(e.target.value as ReportMode)}
          >
            {REPORT_MODES.map((m) => (
              <option key={m} value={m}>
                {MODE_NAMES[m]}
              </option>
            ))}
          </select>
        </label>
        {draft.mode === "template" && (
          <label className="inline-field">
            Template
            <select
              aria-label="Template"
              value={draft.template ?? ""}
              disabled={locked}
              onChange={(e) =>
                void apply(backend.setReportMode(meetingId, "template", e.target.value))
              }
            >
              {draft.template === null && <option value="">Cycle default</option>}
              {(defaults?.templates ?? []).map((t) => (
                <option key={t.name} value={t.name}>
                  {t.name}
                </option>
              ))}
            </select>
          </label>
        )}
        {draft.mode === "by-type" && (
          <label className="inline-field">
            Meeting type
            <select
              aria-label="Meeting type"
              value={draft.meetingType ?? ""}
              disabled={locked}
              onChange={(e) =>
                void apply(backend.setReportMode(meetingId, "by-type", undefined, e.target.value))
              }
            >
              {draft.meetingType === null && <option value="">Choose…</option>}
              {(defaults?.meetingTypes ?? []).map((t) => (
                <option key={t.name} value={t.name}>
                  {t.name}
                </option>
              ))}
            </select>
          </label>
        )}
        <span className="mode-help">{MODE_HELP[draft.mode]}</span>
      </div>

      <section className="panel quiet" aria-label="Summary">
        <div className="panel-body">
          <h3 className="sub-head tight">Summary</h3>
          <p className="summary-text">{draft.summary || "No summary was made."}</p>
        </div>
      </section>

      {draft.warnings.length > 0 && (
        <details className="warnings">
          <summary>Warnings ({draft.warnings.length})</summary>
          <ul aria-label="Warnings">
            {draft.warnings.map((w, i) => (
              <li key={i}>
                {w.message}
                {w.tStartMs !== undefined && ` (${formatClock(w.tStartMs)}`}
                {w.tStartMs !== undefined &&
                  w.tEndMs !== undefined &&
                  ` to ${formatClock(w.tEndMs)}`}
                {w.tStartMs !== undefined && ")"}
              </li>
            ))}
          </ul>
        </details>
      )}

      <div className="pane-toggle" role="group" aria-label="View">
        <button
          className={pane === "items" ? "btn btn-small btn-stop" : "btn btn-small btn-ghost"}
          aria-pressed={pane === "items"}
          onClick={() => setPane("items")}
        >
          Items
        </button>
        <button
          className={pane === "report" ? "btn btn-small btn-stop" : "btn btn-small btn-ghost"}
          aria-pressed={pane === "report"}
          onClick={() => setPane("report")}
        >
          Rendered report
        </button>
        {pane === "items" && draft.mode !== "fixed" && !locked && (
          <button
            className="btn btn-small"
            aria-expanded={editingSections}
            onClick={() => setEditingSections((v) => !v)}
          >
            Edit sections
          </button>
        )}
      </div>

      {pane === "items" && editingSections && draft.mode !== "fixed" && !locked && (
        <SectionsEditor
          draft={draft}
          onSave={async (sections) => {
            const ok = await apply(backend.setDraftSections(meetingId, sections));
            if (ok) setEditingSections(false);
          }}
          onCancel={() => setEditingSections(false)}
        />
      )}

      {pane === "items" && (
        <>
          {!movable && (
            <p className="notice">
              The fixed layout groups items by type. Choose the template, auto or by-type mode to
              work with sections and move items between them.
            </p>
          )}
          <div className="columns" aria-label="Sections">
            {columns.map((col) => (
              <SectionColumn
                key={col.key}
                column={col}
                draft={draft}
                people={people}
                movable={movable && !locked}
                locked={locked}
                dragId={dragId}
                sourceOf={sourceOf}
                transcriptPresent={view.transcriptPresent}
                onUpdate={(id, patch) => apply(backend.updateDraftItem(meetingId, id, patch))}
                onDelete={(id) => void apply(backend.deleteDraftItem(meetingId, id))}
                onMove={(id, sectionId) =>
                  void apply(backend.moveDraftItem(meetingId, id, sectionId))
                }
                onAdd={(type, text) => addItem(type, text, col.id)}
                onToggleSource={(id) => setSourceOf((cur) => (cur === id ? null : id))}
                onJump={(ms) => onOpenTranscript(ms)}
                onDragStart={setDragId}
                onDragEnd={() => setDragId(null)}
                onDrop={(over) => dropOn(col, over)}
                onError={setError}
              />
            ))}
          </div>
          {!movable && !locked && (
            <AddItemForm
              label="Add an item"
              types={ITEM_TYPES}
              onAdd={(type, text) => addItem(type, text, null)}
            />
          )}
        </>
      )}

      {pane === "report" && <ReportPane meetingId={meetingId} view={view} />}

      {(draft.statusProposals?.length ?? 0) > 0 || !locked ? (
        <StatusProposals
          proposals={draft.statusProposals ?? []}
          locked={locked}
          tasks={tasks}
          topics={topics}
          transcriptPresent={view.transcriptPresent}
          onDecide={(id, decision, comment) =>
            apply(backend.setStatusDecision(meetingId, id, decision, comment))
          }
          onAdd={(target, status, comment) =>
            apply(backend.addStatusProposal(meetingId, target, status, comment || undefined))
          }
          onJump={(ms) => onOpenTranscript(ms)}
        />
      ) : null}

      <PublishPanel
        meetingId={meetingId}
        cycleId={draft.cycle}
        draftVersion={view}
        hasNewDraft={view.hasNewDraft}
      />

      {locked && (
        <CorrectionsPanel meetingId={meetingId} cycleId={draft.cycle} items={draft.items} />
      )}
    </>
  );
}

function SectionColumn(props: {
  column: DraftColumn;
  draft: DraftView["draft"];
  people: DraftView["participants"];
  movable: boolean;
  /** The draft is read-only: nothing can be edited, added or removed. */
  locked: boolean;
  dragId: string | null;
  sourceOf: string | null;
  transcriptPresent: boolean;
  onUpdate: (
    id: string,
    patch: { text?: string; assignee?: string | null; due?: string | null },
  ) => Promise<boolean>;
  onDelete: (id: string) => void;
  onMove: (id: string, sectionId: string) => void;
  onAdd: (type: ItemType, text: string) => Promise<boolean>;
  onToggleSource: (id: string) => void;
  onJump: (ms: number) => void;
  onDragStart: (id: string) => void;
  onDragEnd: () => void;
  onDrop: (over: DraftItem | null) => void;
  onError: (message: string | null) => void;
}) {
  const { column, draft, movable, locked, dragId } = props;
  const [over, setOver] = useState(false);
  const droppable = movable && column.id !== null && dragId !== null;
  return (
    <section
      className={over && droppable ? "column column-over" : "column"}
      aria-label={column.title}
      onDragOver={(e) => {
        if (!droppable) return;
        e.preventDefault();
        setOver(true);
      }}
      onDragLeave={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setOver(false);
      }}
      onDrop={(e) => {
        if (!droppable) return;
        e.preventDefault();
        setOver(false);
        props.onDrop(null);
      }}
    >
      <header className="column-head">
        <h3>{column.title}</h3>
        <span className="chip">{column.items.length}</span>
      </header>
      {column.description && <p className="column-desc">{column.description}</p>}
      <ul className="items" aria-label={`Items of ${column.title}`}>
        {column.items.map((item) => (
          <ItemCard
            key={item.id}
            item={item}
            draft={draft}
            people={props.people}
            movable={movable}
            locked={locked}
            dragging={dragId === item.id}
            sourceOpen={props.sourceOf === item.id}
            transcriptPresent={props.transcriptPresent}
            onUpdate={(patch) => props.onUpdate(item.id, patch)}
            onDelete={() => props.onDelete(item.id)}
            onMove={(sectionId) => props.onMove(item.id, sectionId)}
            onToggleSource={() => props.onToggleSource(item.id)}
            onJump={props.onJump}
            onDragStart={() => props.onDragStart(item.id)}
            onDragEnd={props.onDragEnd}
            onDropOn={() => {
              setOver(false);
              props.onDrop(item);
            }}
            onError={props.onError}
          />
        ))}
      </ul>
      {!locked && (
        <AddItemForm
          label={`Add item to ${column.title}`}
          types={column.type ? [column.type] : ITEM_TYPES}
          onAdd={props.onAdd}
          compact
        />
      )}
    </section>
  );
}

function Mark({ level }: { level: Parameters<typeof certaintyMark>[0] }) {
  const m = certaintyMark(level);
  if (m.mark === "") return null;
  return (
    <span className="mark" role="img" aria-label={m.title} title={m.title}>
      {m.mark}
    </span>
  );
}

function ItemCard(props: {
  item: DraftItem;
  draft: DraftView["draft"];
  people: DraftView["participants"];
  movable: boolean;
  locked: boolean;
  dragging: boolean;
  sourceOpen: boolean;
  transcriptPresent: boolean;
  onUpdate: (patch: {
    text?: string;
    assignee?: string | null;
    due?: string | null;
  }) => Promise<boolean>;
  onDelete: () => void;
  onMove: (sectionId: string) => void;
  onToggleSource: () => void;
  onJump: (ms: number) => void;
  onDragStart: () => void;
  onDragEnd: () => void;
  onDropOn: () => void;
  onError: (message: string | null) => void;
}) {
  const { item, draft, people, movable, locked, sourceOpen } = props;
  const [editing, setEditing] = useState(false);
  const [text, setText] = useState(item.text);
  // Set when Escape or a save already ended the edit, so the blur that follows does nothing.
  const finished = useRef(false);
  const targets = moveTargets(draft, item);
  const unverified = isUnverified(item);

  const begin = () => {
    finished.current = false;
    setText(item.text);
    setEditing(true);
  };
  const commit = () => {
    if (finished.current) return;
    finished.current = true;
    const next = text.trim();
    setEditing(false);
    if (next === item.text) return;
    if (next === "") {
      props.onError("The text of an item cannot be empty. Delete the item instead.");
      return;
    }
    void props.onUpdate({ text: next });
  };

  return (
    <li
      className={props.dragging ? "item item-dragging" : "item"}
      data-source-root={sourceOpen ? "" : undefined}
      onDragOver={(e) => {
        if (movable) e.preventDefault();
      }}
      onDrop={(e) => {
        if (!movable) return;
        e.preventDefault();
        e.stopPropagation();
        props.onDropOn();
      }}
    >
      <div className="item-head">
        {movable && (
          <span
            className="drag-handle"
            draggable
            title="Drag to another section"
            aria-hidden="true"
            onDragStart={(e) => {
              e.dataTransfer.effectAllowed = "move";
              e.dataTransfer.setData("text/plain", item.id);
              const card = e.currentTarget.closest("li");
              if (card) e.dataTransfer.setDragImage(card, 12, 12);
              props.onDragStart();
            }}
            onDragEnd={props.onDragEnd}
          >
            ⋮⋮
          </span>
        )}
        <span className={`badge badge-type badge-type-${item.type}`}>
          {ITEM_TYPE_LABELS[item.type]}
        </span>
        {item.edited && (
          <span
            className="edited-dot"
            role="img"
            aria-label="Edited by you"
            title="Edited by you"
          />
        )}
        {unverified && (
          <span className="mark mark-unverified" title="The quote was not found in the transcript">
            unverified quote
          </span>
        )}
      </div>

      {locked ? (
        <p className="seg-text item-text">{item.text}</p>
      ) : editing ? (
        <textarea
          className="seg-edit"
          aria-label={`Text of the item: ${item.text}`}
          value={text}
          rows={2}
          autoFocus
          onChange={(e) => setText(e.target.value)}
          onBlur={commit}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              commit();
            } else if (e.key === "Escape") {
              e.preventDefault();
              e.stopPropagation();
              finished.current = true;
              setEditing(false);
            }
          }}
        />
      ) : (
        <button className="seg-text item-text" title="Click to edit the text" onClick={begin}>
          {item.text}
        </button>
      )}

      {item.type === "task" && (
        <div className="item-fields">
          <label>
            Assignee
            <span className="field-with-mark">
              <select
                aria-label={`Assignee of: ${item.text}`}
                value={item.assignee ?? ""}
                disabled={locked}
                onChange={(e) => void props.onUpdate({ assignee: e.target.value || null })}
              >
                <option value="">Unassigned</option>
                {item.assignee !== null && !people.some((p) => p.id === item.assignee) && (
                  <option value={item.assignee}>{item.assignee}</option>
                )}
                {people.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.name}
                  </option>
                ))}
              </select>
              <Mark level={item.certainty.assignee} />
            </span>
          </label>
          <label>
            Due
            <span className="field-with-mark">
              <DueField
                key={`${item.id}:${item.due}`}
                label={`Due date of: ${item.text}`}
                value={item.due}
                disabled={locked}
                onCommit={(due) => void props.onUpdate({ due })}
              />
              <Mark level={item.certainty.due} />
            </span>
          </label>
        </div>
      )}

      <div className="item-actions">
        {item.source && (
          <button
            className="btn btn-small btn-ghost"
            aria-expanded={sourceOpen}
            aria-label={`Source of: ${item.text}`}
            onClick={props.onToggleSource}
          >
            Source
          </button>
        )}
        {targets.length > 0 && !locked && (
          <select
            className="move-select"
            aria-label={`Move to section: ${item.text}`}
            value=""
            onChange={(e) => e.target.value && props.onMove(e.target.value)}
          >
            <option value="">Move to…</option>
            {targets.map((s) => (
              <option key={s.id} value={s.id}>
                {s.title}
              </option>
            ))}
          </select>
        )}
        {!locked && (
          <button
            className="icon-btn"
            aria-label={`Delete: ${item.text}`}
            title="Delete the item"
            onClick={props.onDelete}
          >
            ×
          </button>
        )}
      </div>

      {sourceOpen && item.source && (
        <div
          className="popover"
          role="dialog"
          aria-label={`Source of: ${item.text}`}
          onKeyDown={(e) => {
            if (e.key === "Escape") {
              e.stopPropagation();
              props.onToggleSource();
            }
          }}
        >
          <blockquote>{item.source.quote}</blockquote>
          <dl className="facts">
            <div>
              <dt>Speaker</dt>
              <dd>{item.source.speaker}</dd>
            </div>
            <div>
              <dt>Time</dt>
              <dd>{formatClock(item.source.tStartMs)}</dd>
            </div>
            <div>
              <dt>Track</dt>
              <dd>{item.source.track}</dd>
            </div>
            <div>
              <dt>Match</dt>
              <dd>
                {matchPercent(item.source.match)}
                {item.source.verified ? " · found" : " · not found"}
              </dd>
            </div>
          </dl>
          <div className="popover-actions">
            <button
              className="btn btn-small"
              autoFocus
              disabled={!props.transcriptPresent}
              onClick={() => props.onJump(item.source!.tStartMs)}
            >
              Jump to transcript
            </button>
            <button className="btn btn-small btn-ghost" onClick={props.onToggleSource}>
              Close
            </button>
          </div>
        </div>
      )}
    </li>
  );
}

/** A date input that keeps what is typed until it is a whole valid date; there is a button to clear it. */
function DueField(props: {
  label: string;
  value: string | null;
  disabled?: boolean;
  onCommit: (due: string | null) => void;
}) {
  const [text, setText] = useState(props.value ?? "");
  return (
    <>
      <input
        type="date"
        aria-label={props.label}
        value={text}
        disabled={props.disabled}
        onChange={(e) => {
          setText(e.target.value);
          if (isIsoDate(e.target.value)) props.onCommit(e.target.value);
        }}
      />
      {props.value !== null && !props.disabled && (
        <button
          className="icon-btn"
          type="button"
          aria-label={props.label.replace("Due date of: ", "Clear due date: ")}
          title="Clear the due date"
          onClick={() => props.onCommit(null)}
        >
          ×
        </button>
      )}
    </>
  );
}

function AddItemForm(props: {
  label: string;
  types: ItemType[];
  onAdd: (type: ItemType, text: string) => Promise<boolean>;
  compact?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [type, setType] = useState<ItemType>(
    props.types.includes("task") ? "task" : props.types[0],
  );
  const [text, setText] = useState("");
  if (!open) {
    return (
      <button
        className="btn btn-small btn-ghost add-item"
        aria-label={props.label}
        onClick={() => setOpen(true)}
      >
        + Add item
      </button>
    );
  }
  const submit = () => {
    if (text.trim() === "") return;
    void props.onAdd(type, text).then((ok) => {
      if (ok) {
        setText("");
        setOpen(false);
      }
    });
  };
  return (
    <form
      className="add-form"
      aria-label={props.label}
      onSubmit={(e) => {
        e.preventDefault();
        submit();
      }}
      onKeyDown={(e) => e.key === "Escape" && setOpen(false)}
    >
      {props.types.length > 1 && (
        <select
          aria-label="Type of the new item"
          value={type}
          onChange={(e) => setType(e.target.value as ItemType)}
        >
          {props.types.map((t) => (
            <option key={t} value={t}>
              {ITEM_TYPE_LABELS[t]}
            </option>
          ))}
        </select>
      )}
      <input
        aria-label="Text of the new item"
        value={text}
        autoFocus
        onChange={(e) => setText(e.target.value)}
      />
      <button className="btn btn-small" type="submit" disabled={text.trim() === ""}>
        Add
      </button>
      <button className="btn btn-small btn-ghost" type="button" onClick={() => setOpen(false)}>
        Cancel
      </button>
    </form>
  );
}

/** Rename, add, remove and reorder the sections; one Save sends them all. */
function SectionsEditor(props: {
  draft: DraftView["draft"];
  onSave: (sections: DraftSection[]) => Promise<void>;
  onCancel: () => void;
}) {
  const { draft } = props;
  const [sections, setSections] = useState<DraftSection[]>(draft.sections);
  const [newTitle, setNewTitle] = useState("");
  const problem = sectionsError(sections);
  const changed = JSON.stringify(sections) !== JSON.stringify(draft.sections);
  const lost = orphanedItems(draft, sections);
  return (
    <section className="panel quiet" aria-label="Edit sections">
      <div className="panel-body">
        <ul className="rows" aria-label="Sections to edit">
          {sections.map((s, i) => (
            <li key={s.id || `new-${i}`} className="section-row">
              <input
                aria-label={`Section ${i + 1} title`}
                value={s.title}
                onChange={(e) => setSections(renameSection(sections, i, e.target.value))}
              />
              <button
                className="btn btn-small btn-ghost"
                aria-label={`Move section ${s.title} up`}
                disabled={i === 0}
                onClick={() => setSections(moveSection(sections, i, -1))}
              >
                Up
              </button>
              <button
                className="btn btn-small btn-ghost"
                aria-label={`Move section ${s.title} down`}
                disabled={i === sections.length - 1}
                onClick={() => setSections(moveSection(sections, i, 1))}
              >
                Down
              </button>
              <button
                className="btn btn-small btn-ghost"
                aria-label={`Remove section ${s.title}`}
                onClick={() => setSections(removeSection(sections, i))}
              >
                Remove
              </button>
            </li>
          ))}
        </ul>
        <form
          className="add-form"
          onSubmit={(e) => {
            e.preventDefault();
            if (newTitle.trim() === "") return;
            setSections(addSection(sections, newTitle));
            setNewTitle("");
          }}
        >
          <input
            aria-label="Title of the new section"
            value={newTitle}
            placeholder="New section"
            onChange={(e) => setNewTitle(e.target.value)}
          />
          <button className="btn btn-small" type="submit" disabled={newTitle.trim() === ""}>
            Add section
          </button>
        </form>
        {lost > 0 && (
          <p className="section-hint">
            {lost === 1 ? "1 item loses its section" : `${lost} items lose their section`} and is
            shown in the last section.
          </p>
        )}
        {problem && (
          <p className="error" role="alert">
            {problem}
          </p>
        )}
        <div className="actions">
          <button className="btn btn-ghost" onClick={props.onCancel}>
            Cancel
          </button>
          <button
            className="btn btn-record"
            disabled={!changed || problem !== null}
            onClick={() => void props.onSave(sections)}
          >
            Save sections
          </button>
        </div>
      </div>
    </section>
  );
}

const VARIANTS: { value: RenderVariant; label: string }[] = [
  { value: "markdown", label: "GitHub Markdown (task checkboxes)" },
  { value: "chat", label: "Chat (Discord, Notion: plain bullets)" },
];

/** The report as the layout renders it, as plain text (escaped, never interpreted as HTML). */
function ReportPane({ meetingId, view }: { meetingId: string; view: DraftView }) {
  const backend = useBackend();
  const [variant, setVariant] = useState<RenderVariant>("markdown");
  const [chat, setChat] = useState<string | null>(null);
  const [copied, setCopied] = useState<string | null>(null);
  const [rendered, setRendered] = useState(false);
  const markdown = view.reportMarkdown;

  useEffect(() => {
    if (variant !== "chat") return;
    let cancelled = false;
    backend
      .renderReport(meetingId, "chat")
      .then((t) => !cancelled && setChat(t))
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [backend, meetingId, variant, markdown]);

  const text = variant === "chat" ? (chat ?? "") : markdown;
  const copy = () => {
    setCopied(null);
    navigator.clipboard
      .writeText(text)
      .then(() => setCopied("Copied"))
      .catch(() => setCopied("Could not copy. Select the text and copy it."));
  };
  return (
    <section className="report-pane" aria-label="Report">
      <div className="report-tools">
        <label className="inline-field">
          Format
          <select
            aria-label="Report format"
            value={variant}
            onChange={(e) => setVariant(e.target.value as RenderVariant)}
          >
            {VARIANTS.map((v) => (
              <option key={v.value} value={v.value}>
                {v.label}
              </option>
            ))}
          </select>
        </label>
        <span className="chip">{MODE_LABELS[view.draft.mode]}</span>
        <span className="pane-toggle" role="group" aria-label="Report view">
          <button
            className={rendered ? "btn btn-small btn-ghost" : "btn btn-small btn-stop"}
            aria-pressed={!rendered}
            onClick={() => setRendered(false)}
          >
            Text
          </button>
          <button
            className={rendered ? "btn btn-small btn-stop" : "btn btn-small btn-ghost"}
            aria-pressed={rendered}
            onClick={() => setRendered(true)}
          >
            Preview
          </button>
        </span>
        {copied && <span role="status">{copied}</span>}
        <button className="btn btn-small" onClick={copy}>
          Copy
        </button>
      </div>
      {rendered ? (
        <MarkdownView
          source={text}
          variant={variant === "chat" ? "discord" : "github"}
          label={variant === "chat" ? "Discord message preview" : "Report preview"}
        />
      ) : (
        <pre className="report-pre" aria-label="Rendered report" tabIndex={0}>
          {text}
        </pre>
      )}
    </section>
  );
}
