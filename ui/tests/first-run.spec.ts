import { type Page, expect, test } from "@playwright/test";

// `jobms=40` makes the simulated downloads and the environment preparation take a moment only.
const url = (setup: string) => `/?view=main&backend=mock&rate=0&jobms=40&setup=${setup}`;

const steps = (page: Page) => page.getByRole("navigation", { name: "Setup steps" });
// The heading carries a decorative "// " before its text, which the accessible name includes.
const title = (page: Page, name: string) =>
  page.getByRole("heading", { level: 2, name: new RegExp(`${name}$`) });
const next = (page: Page) => page.getByRole("button", { name: "Next", exact: true }).click();
const goTo = (page: Page, step: RegExp) => steps(page).getByRole("button", { name: step }).click();

async function leaveSetup(page: Page) {
  await page.getByRole("button", { name: "Leave setup" }).click();
  await expect(page.getByRole("heading", { name: "Meetings", level: 2 })).toBeVisible();
}

test.describe("first-run wizard on the mock backend", () => {
  test("a fresh installation goes through every step to a ready setup", async ({ page }) => {
    await page.goto(url("fresh"));
    await expect(title(page, "Welcome")).toBeVisible();
    await expect(page.getByText("Plan for tens of gigabytes")).toBeVisible();
    await next(page);

    // Processing environment, with progress.
    await expect(title(page, "Processing environment")).toBeVisible();
    await page.getByRole("button", { name: "Prepare environment" }).click();
    await expect(
      page.getByRole("progressbar", { name: "Preparing the environment" }),
    ).toBeVisible();
    await expect(page.getByText("The processing environment is ready.")).toBeVisible();
    await next(page);

    // Speech models and the token.
    await expect(title(page, "Speech models")).toBeVisible();
    await page.getByRole("button", { name: /^Download Whisper turbo/ }).click();
    await expect(
      page.getByRole("progressbar", { name: /^Download of Whisper turbo/ }),
    ).toBeVisible();
    await expect(page.getByRole("button", { name: /^Download Speaker/ })).toBeDisabled();
    await page.getByLabel("Hugging Face token").fill("hf_demo_token_for_the_test");
    await page.getByRole("button", { name: "Store token" }).click();
    await expect(page.getByText("A token is stored")).toBeVisible();
    await expect(page.getByLabel("Hugging Face token")).toHaveValue("");
    const speaker = page.getByRole("button", { name: /^Download Speaker/ });
    await expect(speaker).toBeEnabled();
    await expect(page.getByText("Downloaded", { exact: true })).toBeVisible();
    await speaker.click();
    await expect(page.getByText("Downloaded", { exact: true })).toHaveCount(2);
    await next(page);

    // llama-server: detected, then chosen.
    await expect(title(page, "llama-server")).toBeVisible();
    await expect(page.getByText("llama-server was not found on this computer.")).toBeVisible();
    await page.getByRole("radio", { name: /llama-server\.exe/ }).check();
    await page.getByRole("button", { name: "Use this llama-server" }).click();
    await expect(page.getByText(/^In use:/)).toBeVisible();
    await next(page);

    // Language model: scan, choose, validate.
    await expect(title(page, "Language model")).toBeVisible();
    await page.getByRole("button", { name: "Find model files" }).click();
    await page.getByRole("radio", { name: /gemma-4-12b/ }).check();
    await page.getByRole("button", { name: "Use this model" }).click();
    await expect(page.getByText("A language model is chosen.")).toBeVisible();
    await next(page);

    // Screenshot analysis: download the vision model.
    await expect(title(page, "Screenshot analysis")).toBeVisible();
    await page.getByRole("button", { name: /^Download Screenshot descriptions/ }).click();
    await expect(page.getByText("The screenshot model is downloaded.")).toBeVisible();
    await next(page);

    // Summary: all seven items ready.
    await expect(title(page, "Summary")).toBeVisible();
    const items = page.getByRole("list", { name: "Readiness" }).getByRole("listitem");
    await expect(items).toHaveCount(7);
    await expect(items.getByText("Ready", { exact: true })).toHaveCount(7);
    await expect(page.getByText("Everything needed is ready.")).toBeVisible();
    await page.getByRole("button", { name: "Continue", exact: true }).click();

    // First cycle, then the main window without a readiness banner.
    await expect(title(page, "First cycle")).toBeVisible();
    await page.getByLabel("Cycle id").fill("WEEKLY");
    await page.getByLabel("Cycle name").fill("Weekly sync");
    await page.getByRole("button", { name: "Create first cycle" }).click();
    await expect(page.getByText("Cycle WEEKLY was created.")).toBeVisible();
    await page.getByRole("button", { name: "Finish" }).click();
    await expect(page.getByRole("heading", { name: "Meetings", level: 2 })).toBeVisible();
    await expect(page.getByText(/Setup is incomplete/)).toHaveCount(0);
  });

  test("a missing vision model can be skipped, and the setup is ready then", async ({ page }) => {
    await page.goto(url("no-vision"));
    await goTo(page, /Screenshot analysis/);
    await expect(
      page.getByText(/The screenshot model is not downloaded\. Download it/),
    ).toBeVisible();
    await page.getByRole("button", { name: "Skip screenshot analysis" }).click();
    await expect(page.getByText("Screenshot analysis is off.")).toBeVisible();
    await next(page);
    await expect(page.getByText("Everything needed is ready.")).toBeVisible();
    await expect(
      page.getByRole("list", { name: "Readiness" }).getByText("Skipped", { exact: true }),
    ).toHaveCount(1);
    await page.getByRole("button", { name: "Continue", exact: true }).click();
    // The demo already has cycles: the step says so and only offers to finish.
    await expect(page.getByText(/There are already \d+ cycles\./)).toBeVisible();
    await page.getByRole("button", { name: "Finish", exact: true }).click();
    await expect(page.getByRole("heading", { name: "Meetings", level: 2 })).toBeVisible();
    await expect(page.getByText(/Setup is incomplete/)).toHaveCount(0);
  });

  test("a missing llama-server is explained, and a wrong path is refused with the reason", async ({
    page,
  }) => {
    await page.goto(url("no-llama"));
    await goTo(page, /llama-server/);
    await expect(page.getByText("llama-server was not found on this computer.")).toBeVisible();
    await expect(page.getByText(/Install llama\.cpp, then detect it in the setup\./)).toBeVisible();
    await expect(page.getByText(/None found\./)).toBeVisible();

    const field = page.getByLabel("Path of llama-server.exe");
    const use = page.getByRole("button", { name: "Use this llama-server" });
    await field.fill("C:\\nowhere\\llama-server.exe");
    await use.click();
    await expect(page.getByRole("alert")).toContainText("No file exists at C:\\nowhere");
    await field.fill("C:\\Tools\\other.exe");
    await use.click();
    await expect(page.getByRole("alert")).toContainText("llama-server.exe is expected");

    // The summary still lists it, with a way back to the fix.
    await goTo(page, /Summary/);
    await expect(page.getByRole("button", { name: "Continue anyway" })).toBeVisible();
    await page.getByRole("button", { name: "Fix llama-server" }).click();
    await expect(title(page, "llama-server")).toBeVisible();
  });

  test("without a language model the user can skip reports and sees what that means", async ({
    page,
  }) => {
    await page.goto(url("fresh"));
    await goTo(page, /Language model/);
    await expect(page.getByText(/No language model was chosen/)).toBeVisible();
    await page.getByRole("button", { name: "Skip reports for now" }).click();
    await expect(
      page.getByText(/Meetings are transcribed, but no report is written\./),
    ).toBeVisible();
    await page.getByRole("button", { name: "Turn reports on" }).click();
    await expect(page.getByText(/no report is written/)).toHaveCount(0);
    // A model file that is not a .gguf is refused with the reason.
    await page.getByLabel("Path of the model file").fill("D:\\Models\\notes.txt");
    await page.getByRole("button", { name: "Use this model" }).click();
    await expect(page.getByRole("alert")).toContainText("A .gguf file is expected");
  });

  test("a failed download shows the reason and a retry that finishes it", async ({ page }) => {
    await page.goto(url("download-fails"));
    await goTo(page, /Speech models/);
    await page.getByRole("button", { name: /^Download Whisper turbo/ }).click();
    await expect(page.getByRole("alert")).toContainText("Download failed:");
    await page.getByRole("button", { name: /^Retry download of Whisper turbo/ }).click();
    await expect(
      page.getByRole("progressbar", { name: /^Download of Whisper turbo/ }),
    ).toBeVisible();
    await expect(page.getByText("Downloaded", { exact: true })).toBeVisible();
    await expect(page.getByText(/Download failed/)).toHaveCount(0);
  });

  test("the token field is masked, emptied after storing, and the token never shows", async ({
    page,
  }) => {
    const secret = "hf_masked_token_value_0123456789";
    await page.goto(url("fresh"));
    await goTo(page, /Speech models/);
    const field = page.getByLabel("Hugging Face token");
    await expect(field).toHaveAttribute("type", "password");
    await expect(field).toHaveAttribute("autocomplete", "off");
    await expect(field).toHaveValue("");
    await expect(page.getByRole("button", { name: "Store token" })).toBeDisabled();
    await field.fill(secret);
    await expect(field).toHaveAttribute("type", "password");
    await page.getByRole("button", { name: "Store token" }).click();
    await expect(page.getByText("A token is stored")).toBeVisible();
    await expect(field).toHaveValue("");
    await expect(page.getByText("Clear token")).toBeVisible();
    expect(await page.content()).not.toContain(secret);
    expect(await page.evaluate(() => document.body.innerText)).not.toContain(secret);
    // It is also kept out of the browser storage.
    expect(
      await page.evaluate(
        () => JSON.stringify({ ...localStorage }) + JSON.stringify({ ...sessionStorage }),
      ),
    ).not.toContain(secret);
  });

  test("processing with an incomplete setup opens the step that fixes it", async ({ page }) => {
    await page.goto(url("no-vision"));
    await leaveSetup(page);
    await expect(page.getByText(/Setup is incomplete: Screenshot analysis/)).toBeVisible();

    await page
      .getByRole("button", { name: "Process", exact: true })
      .and(page.locator(":enabled"))
      .first()
      .click();
    await expect(title(page, "Screenshot analysis")).toBeVisible();
    await expect(page.getByRole("alert").first()).toContainText(
      "Processing could not start: The screenshot model is not downloaded.",
    );
  });

  test("the banner opens the setup, and Settings can run it again", async ({ page }) => {
    await page.goto(url("no-llama"));
    await leaveSetup(page);
    await page.getByRole("button", { name: "Open setup" }).click();
    await expect(title(page, "llama-server")).toBeVisible();
    await leaveSetup(page);

    await page.getByRole("button", { name: "Settings", exact: true }).click();
    await page.getByRole("button", { name: "Run setup again" }).click();
    await expect(title(page, "Welcome")).toBeVisible();
  });
});
