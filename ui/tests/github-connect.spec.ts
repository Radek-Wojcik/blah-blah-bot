import { type Page, expect, test } from "@playwright/test";

const CONNECTORS = "/?view=main&backend=mock&rate=0&page=settings&tab=connectors";

interface Hooks {
  setGithubConnect(o: {
    configured?: boolean;
    script?: ({ state: string } & Record<string, unknown>)[];
  }): void;
}
const hooks = <T>(page: Page, run: (h: Hooks) => T): Promise<T> =>
  page.evaluate(`(${run.toString()})(window.__bbbMock)`) as Promise<T>;

test.describe("GitHub: Connect account and token pages", () => {
  test("connects through the device flow and shows who is connected", async ({ page }) => {
    await page.goto(CONNECTORS);
    await page.getByRole("button", { name: "Connect account" }).click();
    await expect(page.getByTestId("github-device-code")).toHaveText("WDJB-MJHT");
    const open = page.getByRole("link", { name: "Open github.com/login/device" });
    await expect(open).toHaveAttribute("href", "https://github.com/login/device");
    await expect(open).toHaveAttribute("target", "_blank");
    await expect(page.getByTestId("github-connected")).toHaveText("Connected as demo-user", {
      timeout: 10_000,
    });
    await expect(page.getByTestId("github-device-code")).toHaveCount(0);
    // The token never reaches the page.
    expect(await page.content()).not.toMatch(/gho_|access_token/);
    await page.getByRole("button", { name: "Disconnect" }).click();
    await expect(page.getByText("Disconnected: the stored token was removed.")).toBeVisible();
  });

  test("a denied approval is explained and the token field stays available", async ({ page }) => {
    await page.goto(CONNECTORS);
    await hooks(page, (h) => h.setGithubConnect({ script: [{ state: "denied" }] }));
    await page.getByRole("button", { name: "Connect account" }).click();
    await expect(page.getByRole("alert").filter({ hasText: "denied" })).toBeVisible({
      timeout: 10_000,
    });
    await expect(page.getByRole("form", { name: "GitHub Token (api.github.com)" })).toBeVisible();
  });

  test("without a client id it says so and points to the token", async ({ page }) => {
    await page.goto(CONNECTORS);
    await hooks(page, (h) => h.setGithubConnect({ configured: false }));
    // The tab asks again when it is opened.
    await page.getByRole("tab", { name: "Preferences" }).click();
    await page.getByRole("tab", { name: "Connectors" }).click();
    await expect(page.getByTestId("github-connect-unconfigured")).toBeVisible();
    await expect(page.getByRole("button", { name: "Connect account" })).toBeDisabled();
  });

  test("connector rows have an Open token page link and GitHub lists its permissions", async ({
    page,
  }) => {
    await page.goto(CONNECTORS);
    const gh = page.getByRole("form", { name: "GitHub Token (api.github.com)" });
    await expect(gh.getByRole("link", { name: "Open token page" })).toHaveAttribute(
      "href",
      "https://github.com/settings/personal-access-tokens/new",
    );
    const hf = page.getByRole("form", { name: "Hugging Face Token" });
    await expect(hf.getByRole("link", { name: "Open token page" })).toHaveAttribute(
      "href",
      "https://huggingface.co/settings/tokens",
    );
    const permissions = page.getByTestId("github-permissions");
    for (const p of ["Administration", "Contents", "Issues"])
      await expect(permissions).toContainText(p);
  });
});
