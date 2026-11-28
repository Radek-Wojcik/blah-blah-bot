import { safeHttpsUrl } from "./publish";

/** Where each provider issues the credential a connector needs (https only, see `safeHttpsUrl`). */
export const GITHUB_TOKEN_PAGE = safeHttpsUrl(
  "https://github.com/settings/personal-access-tokens/new",
);
export const HF_TOKEN_PAGE = safeHttpsUrl("https://huggingface.co/settings/tokens");

/** What a fine-grained GitHub token needs, in the words of GitHub's permission names. */
export const GITHUB_PERMISSIONS = [
  "Administration: Read and write (to create a private repository)",
  "Contents: Read and write (to push meeting reports)",
  "Issues: Read and write (to create and update tasks)",
] as const;

const CLOUD_KEY_PAGES: Record<string, string> = {
  "api.openai.com": "https://platform.openai.com/api-keys",
  "openrouter.ai": "https://openrouter.ai/keys",
  "api.groq.com": "https://console.groq.com/keys",
  "api.mistral.ai": "https://console.mistral.ai/api-keys",
  "generativelanguage.googleapis.com": "https://aistudio.google.com/app/apikey",
};

/** The page where the provider behind a cloud model address issues API keys, or null when unknown. */
export function cloudKeyPage(baseUrl: string): string | null {
  try {
    const host = new URL(baseUrl.trim()).hostname.toLowerCase();
    const known = CLOUD_KEY_PAGES[host];
    return known ? safeHttpsUrl(known) : null;
  } catch {
    return null;
  }
}
