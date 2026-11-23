import { useMemo, useState } from "react";
import { useBackend } from "../backend-context";
import { issuesOf } from "../cycles";
import { errorMessage } from "../hooks";
import { checkNotionMapping } from "../ipc/mock-integrations";
import type {
  CycleIssue,
  CycleTargets,
  NotionAccess,
  NotionMappingProblem,
  NotionProperties,
  NotionSchema,
} from "../ipc/types";
import { NOTION_FIELD_SPECS } from "../publish";
import { RuleChoice } from "./RuleChoice";

type NotionTarget = NonNullable<CycleTargets["notion"]>;

function FieldProblems({ issues, path }: { issues: CycleIssue[]; path: string }) {
  const here = issuesOf(issues, path);
  if (here.length === 0) return null;
  return (
    <small className="field-issue" role="alert">
      {here.map((i) => i.message).join(" ")}
    </small>
  );
}

/**
 * Full Stage 7 Notion target configuration inside the cycle editor: token reference, database id,
 * optional data source picker, "Inspect database & auto-map", property mapping pickers with
 * inline validation, and publishing rule.
 */
export function NotionTargetFields(props: {
  cycleId: string;
  notion: NotionTarget;
  /** True when token or database differs from the last saved cycle. */
  connectionDirty: boolean;
  issues: CycleIssue[];
  onChange: (next: NotionTarget) => void;
}) {
  const { cycleId, notion, connectionDirty, issues, onChange } = props;
  const backend = useBackend();
  const [inspecting, setInspecting] = useState(false);
  const [access, setAccess] = useState<NotionAccess | null>(null);
  const [schema, setSchema] = useState<NotionSchema | null>(null);
  const [inspectStatus, setInspectStatus] = useState<{
    text: string;
    kind: "status" | "error";
  } | null>(null);

  const patch = (p: Partial<NotionTarget>) => onChange({ ...notion, ...p });
  const patchProps = (p: Partial<NotionProperties>) =>
    patch({ properties: { ...notion.properties, ...p } });

  const liveProblems = useMemo<NotionMappingProblem[]>(() => {
    if (schema) return checkNotionMapping(notion.properties, schema.properties);
    if (notion.properties.status && !notion.properties.statusValue?.trim()) {
      return [{ field: "statusValue", message: "Choose the status a new page gets." }];
    }
    return [];
  }, [notion.properties, schema]);

  const problemOf = (field: keyof NotionProperties) =>
    liveProblems.find((p) => p.field === field)?.message ?? null;

  const inspect = () => {
    if (connectionDirty) {
      setInspectStatus({
        text: "Save the cycle first so the token reference and database id are stored, then inspect.",
        kind: "error",
      });
      return;
    }
    setInspecting(true);
    setInspectStatus(null);
    backend
      .testNotionAccess(cycleId)
      .then(async (a) => {
        setAccess(a);
        if (!a.ok) {
          setSchema(null);
          setInspectStatus({ text: a.message, kind: "error" });
          return;
        }
        const s = await backend.notionSchema(cycleId);
        setSchema(s);
        const nextDataSource =
          a.dataSources.length > 1 ? (notion.dataSource ?? s.dataSourceId) : notion.dataSource;
        if (checkNotionMapping(s.proposed, s.properties).length === 0) {
          await backend
            .setNotionMapping(cycleId, nextDataSource ?? null, s.proposed)
            .catch(() => undefined);
        }
        onChange({
          ...notion,
          ...(nextDataSource ? { dataSource: nextDataSource } : {}),
          properties: { ...s.proposed },
        });
        setInspectStatus({
          text: `${a.message} Loaded ${s.properties.length} properties and applied suggested mapping.`,
          kind: "status",
        });
      })
      .catch((e) => {
        setSchema(null);
        setInspectStatus({ text: errorMessage(e), kind: "error" });
      })
      .finally(() => setInspecting(false));
  };

  const dataSources = access?.dataSources ?? [];

  return (
    <>
      <label>
        Integration token reference
        <input
          value={notion.token}
          placeholder="cred:notion/nomad"
          onChange={(e) => patch({ token: e.target.value })}
        />
        <small>
          The name of a secret in the Windows Credential Manager, written cred:name. Store the token
          in Settings &gt; Publishing.
        </small>
        <FieldProblems issues={issues} path="targets.notion.token" />
      </label>

      <label>
        Database id
        <input
          value={notion.database}
          placeholder="0123456789abcdef0123456789abcdef"
          onChange={(e) => patch({ database: e.target.value })}
        />
        <small>
          32 hexadecimal digits from the Notion database link (dashes allowed). Share the database
          with your integration in Notion (&hellip; &gt; Connections).
        </small>
        <FieldProblems issues={issues} path="targets.notion.database" />
      </label>

      {dataSources.length > 0 ? (
        <label>
          Data source (optional)
          <select
            aria-label="Notion data source"
            value={notion.dataSource ?? ""}
            onChange={(e) => patch({ dataSource: e.target.value || undefined })}
          >
            <option value="">Default (single data source)</option>
            {dataSources.map((ds) => (
              <option key={ds.id} value={ds.id}>
                {ds.name} ({ds.id})
              </option>
            ))}
          </select>
          <small>Only required when the Notion database contains multiple data sources.</small>
          <FieldProblems issues={issues} path="targets.notion.data_source" />
        </label>
      ) : (
        <label>
          Data source id (optional)
          <input
            value={notion.dataSource ?? ""}
            placeholder="Leave empty for single-source databases"
            onChange={(e) => patch({ dataSource: e.target.value || undefined })}
          />
          <small>Needed only when the database has more than one data source.</small>
          <FieldProblems issues={issues} path="targets.notion.data_source" />
        </label>
      )}

      <div className="actions">
        <button
          className="btn btn-small btn-ghost"
          type="button"
          disabled={inspecting}
          onClick={inspect}
        >
          {inspecting ? "Inspecting…" : "Inspect database & auto-map"}
        </button>
        {schema && (
          <button
            className="btn btn-small btn-ghost"
            type="button"
            onClick={() => patch({ properties: { ...schema.proposed } })}
          >
            Reset to suggested mapping
          </button>
        )}
      </div>

      {inspectStatus && (
        <p
          className={inspectStatus.kind === "error" ? "error" : "secret-result"}
          role={inspectStatus.kind === "error" ? "alert" : "status"}
        >
          {inspectStatus.text}
        </p>
      )}

      <div className="notion-mapping" aria-label="Notion property mapping">
        <span className="field-label">Property mapping</span>
        <p className="section-hint">
          Choose which Notion database property each task field writes to. Status is set only when a
          page is first created so manual updates in Notion are preserved.
        </p>
        <div className="mapping-grid">
          {NOTION_FIELD_SPECS.map((spec) => {
            const current = notion.properties[spec.field] ?? "";
            const inlineErr = problemOf(spec.field);
            return (
              <label key={spec.field}>
                {spec.label}
                {schema ? (
                  <select
                    aria-label={`Notion ${spec.label} property`}
                    value={current}
                    onChange={(e) => {
                      const val = e.target.value;
                      if (spec.required) {
                        patchProps({ [spec.field]: val });
                      } else if (spec.field === "status") {
                        patchProps({
                          status: val || undefined,
                          statusValue: val
                            ? (notion.properties.statusValue ?? "Not started")
                            : undefined,
                        });
                      } else {
                        patchProps({ [spec.field]: val || undefined });
                      }
                    }}
                  >
                    {!spec.required && <option value="">Not mapped</option>}
                    {current !== "" && !schema.properties.some((p) => p.name === current) && (
                      <option value={current}>{current} (not in database)</option>
                    )}
                    {schema.properties.map((p) => (
                      <option key={p.name} value={p.name}>
                        {p.name} ({p.type})
                      </option>
                    ))}
                  </select>
                ) : (
                  <input
                    aria-label={`Notion ${spec.label} property`}
                    value={current}
                    placeholder={spec.placeholder}
                    onChange={(e) => {
                      const val = e.target.value;
                      if (spec.required) {
                        patchProps({ [spec.field]: val });
                      } else {
                        patchProps({ [spec.field]: val || undefined });
                      }
                    }}
                  />
                )}
                <small>{spec.help}</small>
                {inlineErr && (
                  <small className="field-issue" role="alert">
                    {inlineErr}
                  </small>
                )}
              </label>
            );
          })}

          {(notion.properties.status ?? "").trim() !== "" && (
            <label>
              Initial status value
              <input
                aria-label="Notion initial status value"
                value={notion.properties.statusValue ?? ""}
                placeholder="Not started"
                onChange={(e) => patchProps({ statusValue: e.target.value || undefined })}
              />
              <small>
                Assigned only when creating a new page; existing pages keep their status.
              </small>
              {problemOf("statusValue") && (
                <small className="field-issue" role="alert">
                  {problemOf("statusValue")}
                </small>
              )}
              <FieldProblems issues={issues} path="targets.notion.properties.status_value" />
            </label>
          )}
        </div>
      </div>

      <div className="field-group">
        <span className="field-label">When to publish</span>
        <RuleChoice
          name="Notion"
          target="notion"
          value={notion.publish}
          onChange={(publish) => patch({ publish })}
        />
      </div>
    </>
  );
}
