import { type Page, expect } from "@playwright/test";

/** Presses Record in the widget recorder panel and waits until recording begins. */
export async function recordFromGate(page: Page) {
  await page.getByRole("button", { name: "Record" }).click();
  await expect(page.getByText("Recording", { exact: true })).toBeVisible();
}
