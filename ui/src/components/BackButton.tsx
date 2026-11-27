export interface BackButtonProps {
  label?: string;
  onClick: () => void;
}

export function BackButton({ label = "Back", onClick }: BackButtonProps) {
  return (
    <button type="button" className="btn btn-small btn-ghost back-btn" onClick={onClick}>
      <svg
        width="14"
        height="14"
        viewBox="0 0 16 16"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.8"
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden="true"
      >
        <path d="M10 13L5 8l5-5" />
      </svg>
      <span>{label}</span>
    </button>
  );
}
