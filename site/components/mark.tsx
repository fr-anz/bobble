/** The app icon's face: a round head with one plain eye and one spark eye, over a jaw. */
const HEAD =
  "M6.252 26A15 15 0 1 1 33.748 26ZM11.3 19.5a2.2 2.6 0 1 0 4.4 0a2.2 2.6 0 1 0-4.4 0ZM25 7.5Q25 12 29.5 12Q25 12 25 16.5Q25 12 20.5 12Q25 12 25 7.5Z";
const JAW = "M7.01 27.5A15 15 0 0 0 32.99 27.5Z";

export function Mark({ className, animated = false }: { className?: string; animated?: boolean }) {
  const part = animated ? "mark-tile" : undefined;
  return (
    <svg className={className} viewBox="4 4 32 32" aria-hidden="true">
      <path className={part} fillRule="evenodd" d={HEAD} />
      <path className={part} style={animated ? { animationDelay: "120ms" } : undefined} fillOpacity={0.4} d={JAW} />
    </svg>
  );
}
