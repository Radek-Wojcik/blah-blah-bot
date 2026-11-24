import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { CycleTasks, StatusDecision, StatusProposal, Topic } from "../ipc/types";
import { MarkdownView } from "./MarkdownView";
import { StatusProposals } from "./StatusProposals";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let host: HTMLDivElement;
let root: Root;

beforeEach(() => {
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
});

afterEach(() => {
  act(() => root.unmount());
  host.remove();
});

const render = (node: React.ReactNode) => act(async () => root.render(node));

const click = (el: Element | null) =>
  act(async () => {
    el?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });

const proposal = (patch: Partial<StatusProposal> = {}): StatusProposal => ({
  id: "s1",
  target: "#12",
  status: "done",
  reason: "Alex said the drawings are finished.",
  comment: null,
  source: {
    track: "app-discord",
    speaker: "alex",
    tStartMs: 250_000,
    tEndMs: 262_000,
    quote: "The antenna mount drawings are finished and uploaded.",
    verified: true,
    match: 0.97,
  },
  decision: "proposed",
  edited: false,
  ...patch,
});

const tasks: CycleTasks = {
  cycleId: "NOMAD",
  repo: "demo-org/nomad-cycles",
  asOf: "2026-11-13",
  open: [
    {
      number: 12,
      title: "Finish the antenna mount drawings",
      assignee: "jan-demo",
      url: "https://github.com/demo-org/nomad-cycles/issues/12",
      meeting: "m4",
    },
  ],
  done: [],
};

const topics: Topic[] = [];

type Decide = (id: string, decision: StatusDecision, comment?: string) => Promise<boolean>;
const decider = () => vi.fn<Decide>().mockResolvedValue(true);

function mount(
  proposals: StatusProposal[],
  extra: { locked?: boolean; onDecide?: ReturnType<typeof decider> } = {},
) {
  const onDecide = extra.onDecide ?? decider();
  return {
    onDecide,
    done: render(
      <StatusProposals
        proposals={proposals}
        locked={extra.locked ?? false}
        tasks={tasks}
        topics={topics}
        transcriptPresent
        onDecide={onDecide}
        onAdd={vi.fn().mockResolvedValue(true)}
        onJump={vi.fn()}
      />,
    ),
  };
}

const button = (name: string): HTMLButtonElement =>
  host.querySelector<HTMLButtonElement>(`button[aria-label="${name}"]`)!;

describe("status proposals", () => {
  it("shows the evidence quote, the issue link and what approving does", async () => {
    await mount([proposal()]).done;
    expect(host.querySelector("blockquote")?.textContent).toBe(
      "The antenna mount drawings are finished and uploaded.",
    );
    const link = host.querySelector<HTMLAnchorElement>("a.proposal-target")!;
    expect(link.textContent).toBe("#12");
    expect(link.href).toBe("https://github.com/demo-org/nomad-cycles/issues/12");
    expect(link.rel).toContain("noreferrer");
    expect(host.textContent).toContain("Finish the antenna mount drawings");
    expect(host.textContent).toContain("Closes issue #12 on GitHub and adds a comment");
    expect(host.textContent).toContain("alex · 00:04:10 · match 97%");
    expect(host.textContent).toContain("Waiting for your decision");
    expect(host.textContent).toContain("0 of 1 approved, 1 waiting for your decision");
  });

  it("marks a quote that was not found in the transcript", async () => {
    const p = proposal();
    await mount([{ ...p, source: { ...p.source!, verified: false, match: 0.4 } }]).done;
    expect(host.textContent).toContain("unverified quote");
  });

  it("never approves by itself: a proposal starts undecided", async () => {
    const { onDecide, done } = mount([proposal()]);
    await done;
    expect(onDecide).not.toHaveBeenCalled();
    expect(button("Approve #12").getAttribute("aria-pressed")).toBe("false");
  });

  it("approves and rejects with the comment, and takes a decision back by pressing it again", async () => {
    const onDecide = decider();
    await mount([proposal()], { onDecide }).done;
    await click(button("Approve #12"));
    expect(onDecide).toHaveBeenLastCalledWith("s1", "approved", "");
    await click(button("Reject #12"));
    expect(onDecide).toHaveBeenLastCalledWith("s1", "rejected", "");
    await render(
      <StatusProposals
        proposals={[proposal({ decision: "approved", comment: "ok", edited: true })]}
        locked={false}
        tasks={tasks}
        topics={topics}
        transcriptPresent
        onDecide={onDecide}
        onAdd={vi.fn()}
        onJump={vi.fn()}
      />,
    );
    expect(button("Approve #12").getAttribute("aria-pressed")).toBe("true");
    expect(host.textContent).toContain("1 of 1 approved");
    await click(button("Approve #12"));
    expect(onDecide).toHaveBeenLastCalledWith("s1", "proposed", "ok");
  });

  it("is read-only once the meeting is published", async () => {
    const onDecide = decider();
    await mount([proposal({ decision: "approved" })], { locked: true, onDecide }).done;
    expect(button("Approve #12").disabled).toBe(true);
    expect(button("Reject #12").disabled).toBe(true);
    expect(host.querySelector("textarea")?.disabled).toBe(true);
    expect(host.textContent).toContain("The decisions are read-only");
    expect(host.textContent).not.toContain("Add a status change");
    await click(button("Approve #12"));
    expect(onDecide).not.toHaveBeenCalled();
  });

  it("says so when the model found nothing", async () => {
    await mount([]).done;
    expect(host.textContent).toContain("The model found no status changes.");
  });
});

describe("rendered Markdown", () => {
  it("renders the elements of a report and keeps tags as text", async () => {
    await render(
      <MarkdownView
        variant="github"
        label="Report preview"
        source={
          "# Title\n\n- [ ] Task one\n- [x] Task two\n\n<script>alert(1)</script> and **bold**"
        }
      />,
    );
    expect(host.querySelector("h2")?.textContent).toBe("Title");
    expect(host.querySelectorAll('input[type="checkbox"]')).toHaveLength(2);
    expect(host.querySelectorAll<HTMLInputElement>("input:checked")).toHaveLength(1);
    expect(host.querySelector("script")).toBeNull();
    expect(host.textContent).toContain("<script>alert(1)</script>");
    expect(host.querySelector("strong")?.textContent).toBe("bold");
  });

  it("makes a link of http(s) only", async () => {
    await render(
      <MarkdownView
        variant="discord"
        label="Discord message preview"
        source={"[ok](https://example.org/a) [bad](javascript:alert(1)) [file](transcript.md)"}
      />,
    );
    const links = [...host.querySelectorAll("a")];
    expect(links.map((a) => a.getAttribute("href"))).toEqual(["https://example.org/a"]);
    expect(host.textContent).toContain("Blah Blah Bot");
    expect(host.querySelector('[aria-label="Discord message preview"]')).not.toBeNull();
  });
});
