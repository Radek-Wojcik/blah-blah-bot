import { type Page, expect } from "@playwright/test";

/** Presses Record in the widget and confirms the report mode gate (the last report is preselected). */
export async function recordFromGate(page: Page) {
  await page.getByRole("button", { name: "Record" }).click();
  const gate = page.getByRole("dialog", { name: "Choose the report for this recording" });
  await expect(gate).toBeVisible();
  await gate.getByRole("button", { name: "Start recording" }).click();
  await expect(gate).toHaveCount(0);
}
