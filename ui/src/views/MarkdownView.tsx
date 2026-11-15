import { Fragment, type ReactNode, useMemo } from "react";
import { type Block, type Inline, parseMarkdown } from "../markdown";

function inline(nodes: Inline[]): ReactNode {
  return nodes.map((n, i) => {
    switch (n.kind) {
      case "text":
        return <Fragment key={i}>{n.text}</Fragment>;
      case "bold":
        return <strong key={i}>{inline(n.children)}</strong>;
      case "italic":
        return <em key={i}>{inline(n.children)}</em>;
      case "code":
        return <code key={i}>{n.text}</code>;
      case "link":
        return (
          <a key={i} href={n.href} target="_blank" rel="noreferrer noopener">
            {inline(n.children)}
          </a>
        );
      case "ref":
        return (
          <span key={i} className="md-ref" title={`Link to ${n.target}`}>
            {inline(n.children)}
          </span>
        );
    }
  });
}

function block(b: Block, i: number): ReactNode {
  switch (b.kind) {
    case "heading": {
      const Tag = `h${Math.min(6, b.level + 1)}` as "h2";
      return (
        <Tag key={i} className={`md-h md-h${b.level}`}>
          {inline(b.children)}
        </Tag>
      );
    }
    case "paragraph":
      return <p key={i}>{inline(b.children)}</p>;
    case "quote":
      return <blockquote key={i}>{inline(b.children)}</blockquote>;
    case "rule":
      return <hr key={i} />;
    case "code":
      return (
        <pre key={i} className="md-code">
          <code>{b.text}</code>
        </pre>
      );
    case "list": {
      const Tag = b.ordered ? "ol" : "ul";
      return (
        <Tag key={i} className={b.items.some((x) => x.checked !== null) ? "md-tasks" : undefined}>
          {b.items.map((item, j) => (
            <li key={j}>
              {item.checked !== null && (
                <input type="checkbox" checked={item.checked} disabled aria-label="Task" readOnly />
              )}
              {inline(item.children)}
            </li>
          ))}
        </Tag>
      );
    }
  }
}

/**
 * A Markdown text as it reads once rendered: headings, lists with task checkboxes, links. Built
 * from elements, never from HTML, so a transcript cannot add markup to it. `variant` only
 * changes the frame: a document as GitHub shows it, or a message as Discord shows it.
 */
export function MarkdownView(props: {
  source: string;
  variant: "github" | "discord";
  label: string;
}) {
  const blocks = useMemo(() => parseMarkdown(props.source), [props.source]);
  if (props.variant === "discord") {
    return (
      <div className="discord-message" role="region" aria-label={props.label} tabIndex={0}>
        <span className="discord-avatar" aria-hidden="true">
          B
        </span>
        <div className="discord-body">
          <p className="discord-author">
            <strong>Blah Blah Bot</strong> <span className="discord-tag">APP</span>
          </p>
          <div className="md md-discord">{blocks.map(block)}</div>
        </div>
      </div>
    );
  }
  return (
    <article className="md md-github" aria-label={props.label} tabIndex={0}>
      {blocks.map(block)}
    </article>
  );
}
