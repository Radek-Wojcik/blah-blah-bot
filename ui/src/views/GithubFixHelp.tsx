import { GITHUB_PERMISSIONS, GITHUB_TOKEN_PAGE } from "../tokenPages";

/**
 * Whether the message of a failed repository creation is about the saved token (rejected, or
 * not allowed to create repositories), which the person fixes in Settings > Connectors.
 */
export function needsTokenHelp(message: string): boolean {
  return /repository creation permission|saved token/i.test(message);
}

/** "How to fix" for a token that cannot create repositories: the permissions and the token page. */
export function GithubFixHelp() {
  return (
    <details className="notice" data-testid="github-fix-help">
      <summary>How to fix</summary>
      <p>
        Connect your account in Settings &gt; Connectors &gt; GitHub, or create a fine-grained token
        with these repository permissions and store it there:
      </p>
      <ul>
        {GITHUB_PERMISSIONS.map((p) => (
          <li key={p}>{p}</li>
        ))}
      </ul>
      {GITHUB_TOKEN_PAGE && (
        <p>
          <a href={GITHUB_TOKEN_PAGE} target="_blank" rel="noreferrer noopener">
            Open the GitHub token page
          </a>
        </p>
      )}
      <p>Then open the project and link or create its repository again.</p>
    </details>
  );
}
