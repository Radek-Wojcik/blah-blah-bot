import { act } from "react";
import { createRoot } from "react-dom/client";
import { describe, expect, it } from "vitest";
import { GithubFixHelp, needsTokenHelp } from "./GithubFixHelp";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

describe("GithubFixHelp", () => {
  it("recognises the messages about the saved token", () => {
    expect(
      needsTokenHelp(
        "the saved token is not allowed to create repositories in your GitHub account (it needs the repository creation permission)",
      ),
    ).toBe(true);
    expect(needsTokenHelp("GitHub rejected the saved token: it is wrong, expired or revoked")).toBe(
      true,
    );
    expect(needsTokenHelp("GitHub could not be reached (offline)")).toBe(false);
  });

  it("explains the permissions and links to the token page over https", async () => {
    const host = document.createElement("div");
    const root = createRoot(host);
    await act(async () => root.render(<GithubFixHelp />));
    expect(host.querySelector("summary")?.textContent).toBe("How to fix");
    expect(host.textContent).toContain("Administration");
    const link = host.querySelector("a")!;
    expect(link.getAttribute("href")).toBe(
      "https://github.com/settings/personal-access-tokens/new",
    );
    expect(link.rel).toContain("noopener");
    act(() => root.unmount());
  });
});
