import { expect, test } from "@playwright/test";

const MAIN = "/?view=main&backend=mock";

test.describe("global voice profiles with per-project badges", () => {
  test("enroll global profile, link/unlink project badge, duplicate hint, and merge", async ({
    page,
  }) => {
    await page.goto(`${MAIN}&rate=0`);

    await page.getByRole("button", { name: "Settings" }).click();
    await page.getByRole("tab", { name: "Voice profiles" }).click();

    const enrollForm = page.getByRole("form", { name: "Add voice profile manually" });
    await enrollForm.getByLabel("Person name").fill("Sam Cross");
    await enrollForm.getByLabel("Project").selectOption("");
    await enrollForm.getByLabel("15–30 s audio/video sample path").fill("C:\\Audio\\sam1.wav");
    await enrollForm.getByRole("button", { name: "Enroll voice profile" }).click();
    await expect(page.getByText("Enrolled voice profile for Sam Cross.")).toBeVisible();

    // Enroll a second person with the same name in NOMAD to trigger duplicate hint.
    await enrollForm.getByLabel("Person name").fill("Sam Cross");
    await enrollForm.getByLabel("Project").selectOption("NOMAD");
    await enrollForm.getByLabel("15–30 s audio/video sample path").fill("C:\\Audio\\sam2.wav");
    await enrollForm.getByRole("button", { name: "Enroll voice profile" }).click();
    await expect(page.getByText("Possible duplicate name").first()).toBeVisible();

    // Remove NOMAD badge from the NOMAD card -> falls back to Global.
    await page.getByRole("button", { name: "Remove NOMAD from Sam Cross" }).click();

    // Add one Sam Cross to NOMAD via + Add to project.
    const addSelect = page.getByLabel("Add Sam Cross to project").first();
    await addSelect.selectOption("NOMAD");

    // Merge the two Sam Cross profiles via Same person as...
    const mergeSelect = page.getByLabel("Same person as Sam Cross").first();
    const optionValue = await mergeSelect
      .locator("option", { hasText: "Sam Cross" })
      .first()
      .getAttribute("value");
    expect(optionValue).toBeTruthy();
    await mergeSelect.selectOption(optionValue!);
    const dialog = page.getByRole("dialog");
    await expect(dialog.getByText("Merge these voice profiles?")).toBeVisible();
    await dialog.getByRole("button", { name: "Merge profiles" }).click();

    await expect(page.getByText("Possible duplicate name")).toHaveCount(0);
  });
});
